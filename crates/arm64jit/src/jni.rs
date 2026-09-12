//! Guest-visible Java Native Interface (JNI) environment + JavaVM for the JIT.
//!
//! Ports the QEMU path's `jni_shim.c` JNI function table. On Android `JNI_OnLoad`
//! receives a `JavaVM*`; it calls `vm->GetEnv(&env, JNI_VERSION_1_6)` to obtain a
//! `JNIEnv*` whose word0 is a pointer to the `JNINativeInterface` function table,
//! then calls `env->FindClass/GetStaticMethodID/NewStringUTF/...` through it.
//!
//! In the JIT host thunks live in guest-address space and guest==host, so the
//! JNI table we build in guest memory holds host-thunk guest addresses directly.
//! Each JNINative slot maps to a host Rust stub with the generic `HostCall` ABI
//! (8 u64 args -> u64). This is the C file's `jni_table`/`vm_table` in
//! thunk-address form.
//!
//! # Slot indices are the OFFICIAL Android JNI ABI
//! libroblox.so is compiled against Android's `jni.h`, so it indexes the
//! function table with the official `JNINativeInterface` word offsets
//! (reserved0..3 = 0..3, GetVersion=4, FindClass=6, GetMethodID=33,
//! GetFieldID=94, GetStaticMethodID=113, NewStringUTF=167,
//! GetStringUTFChars=169, RegisterNatives=199, GetJavaVM=203). The JIT table
//! MUST use these exact offsets or a guest call lands on the wrong stub. (The
//! QEMU shim's earlier 36/193/197/102 etc. were unvalidated guesses — its only
//! end-to-end-validated slots were GetVersion/FindClass/GetStaticMethodID,
//! which coincidentally match the official offsets.)

use crate::jit::{register_host_call_auto, HostCall, HostJniF32};
use std::alloc::{alloc_zeroed, Layout};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const JNI_VERSION_1_6: i32 = 0x0001_0006;
const JNI_SLOTS: usize = 256;
const VM_SLOTS: usize = 8;

// Official JNINativeInterface word offsets (the ones the guest actually uses).
const GET_VERSION: usize = 4;
const FIND_CLASS: usize = 6;
const THROW: usize = 13;
const THROW_NEW: usize = 14;
const NEW_GLOBAL_REF: usize = 21;
const DEL_GLOBAL_REF: usize = 22;
const DEL_LOCAL_REF: usize = 23;
const GET_METHOD_ID: usize = 33;
const GET_FIELD_ID: usize = 94;
const GET_OBJ_FIELD: usize = 95;
const GET_BOOLEAN_FIELD: usize = 96;
const GET_INT_FIELD: usize = 100;
const SET_OBJ_FIELD: usize = 104;
const SET_BOOLEAN_FIELD: usize = 105;
const GET_STATIC_METHOD_ID: usize = 113;
const NEW_STRING_UTF: usize = 167;
const GET_STRING_UTF_LEN: usize = 168;
const GET_STRING_UTF_CHARS: usize = 169;
const GET_ARRAY_LEN: usize = 171;
const NEW_OBJECT_ARRAY: usize = 172;
const GET_OBJ_ARR_ELEM: usize = 173;
const SET_OBJ_ARR_ELEM: usize = 174;
// Primitive-array creation (indices verified against Android NDK r26b jni.h —
// the JDK table differs only by Set*ArrayRegion placement, but the NDK is what
// libroblox.so indexes against. All offsets below are the authoritative NDK.)
const NEW_BYTE_ARRAY: usize = 176;
const NEW_INT_ARRAY: usize = 179;
const GET_BYTE_ARRAY_ELEMENTS: usize = 184;
const GET_INT_ARRAY_ELEMENTS: usize = 187;
const RELEASE_BYTE_ARRAY_ELEMENTS: usize = 192;
const RELEASE_INT_ARRAY_ELEMENTS: usize = 195;
const GET_BYTE_ARRAY_REGION: usize = 200;
const GET_INT_ARRAY_REGION: usize = 203;
const SET_BYTE_ARRAY_REGION: usize = 208;
const SET_INT_ARRAY_REGION: usize = 211;
const REGISTER_NATIVES: usize = 215;
const GET_JAVA_VM: usize = 219;
// More official JNINativeInterface offsets (object-model / monitor / exception /
// ref / static-field / call-method / direct-buffer surface), verified against
// Android NDK jni.h. These previously fell to the `voidp` default returning 0;
// wiring them to typed stubs (and real fake-object backing) makes guest JNI
// paths that check `if (!ref / !clazz / !buf) fail` pass instead of aborting.
const IS_ASSIGNABLE_FROM: usize = 11;
const EXCEPTION_OCCURRED: usize = 15;
const EXCEPTION_DESCRIBE: usize = 16;
const EXCEPTION_CLEAR: usize = 17;
const PUSH_LOCAL_FRAME: usize = 19;
const POP_LOCAL_FRAME: usize = 20;
const IS_SAME_OBJECT: usize = 24;
const NEW_LOCAL_REF: usize = 25;
const ENSURE_LOCAL_CAPACITY: usize = 26;
const ALLOC_OBJECT: usize = 27;
const NEW_OBJECT: usize = 28;
const GET_OBJECT_CLASS: usize = 31;
const IS_INSTANCE_OF: usize = 32;
const CALL_OBJECT_METHOD: usize = 34;
const CALL_BOOLEAN_METHOD: usize = 37;
const CALL_INT_METHOD: usize = 49;
const CALL_LONG_METHOD: usize = 52;
const CALL_FLOAT_METHOD: usize = 55;
const CALL_VOID_METHOD: usize = 61;
const CALL_STATIC_OBJECT_METHOD: usize = 114;
const CALL_STATIC_BOOLEAN_METHOD: usize = 117;
const CALL_STATIC_INT_METHOD: usize = 129;
const CALL_STATIC_VOID_METHOD: usize = 141;
const GET_STATIC_FIELD_ID: usize = 144;
const GET_STATIC_OBJECT_FIELD: usize = 145;
const GET_STATIC_INT_FIELD: usize = 150;
const SET_STATIC_OBJECT_FIELD: usize = 154;
const SET_STATIC_INT_FIELD: usize = 159;
const RELEASE_STRING_UTF_CHARS: usize = 170;
const UNREGISTER_NATIVES: usize = 216;
const MONITOR_ENTER: usize = 217;
const MONITOR_EXIT: usize = 218;
const GET_STRING_UTF_REGION: usize = 221;
const NEW_WEAK_GLOBAL_REF: usize = 226;
const DELETE_WEAK_GLOBAL_REF: usize = 227;
const EXCEPTION_CHECK: usize = 228;
const NEW_DIRECT_BYTE_BUFFER: usize = 229;
const GET_DIRECT_BUFFER_ADDRESS: usize = 230;
const GET_DIRECT_BUFFER_CAPACITY: usize = 231;
// Official JNIVMInterface (JNIInvokeInterface) word offsets. The struct:
//   reserved0..2 = 0..2, DestroyJavaVM=3, AttachCurrentThread=4,
//   DetachCurrentThread=5, GetEnv=6, AttachCurrentThreadAsDaemon=7.
// Verified against host java-21 jni.h AND the guest disasm (JNI_OnLoad does
// `ldr x8,[vm] ; ldr x8,[x8,#48] ; blr x8` = byte 48 = word 6 = GetEnv).
const VM_GET_ENV: usize = 6;

/// Human names for each filled JNIEnv/JavaVM slot, keyed by the slot-const
/// value it occupies in the JNIEnv function table. Used to give the JIT_TRACE
/// hostcall dumper a readable name for every JNI function the engine reaches
/// (instead of an anonymous `slotN`), which makes the real-boot run-log
/// legible when GameActivity init churns JNI calls.
const JNI_METHOD_NAMES: &[(&str, &usize)] = &[
    ("JNIEnv.GetVersion", &GET_VERSION),
    ("JNIEnv.FindClass", &FIND_CLASS),
    ("JNIEnv.Throw", &THROW),
    ("JNIEnv.ThrowNew", &THROW_NEW),
    ("JNIEnv.NewGlobalRef", &NEW_GLOBAL_REF),
    ("JNIEnv.DeleteGlobalRef", &DEL_GLOBAL_REF),
    ("JNIEnv.DeleteLocalRef", &DEL_LOCAL_REF),
    ("JNIEnv.GetMethodID", &GET_METHOD_ID),
    ("JNIEnv.GetFieldID", &GET_FIELD_ID),
    ("JNIEnv.GetObjectField", &GET_OBJ_FIELD),
    ("JNIEnv.GetBooleanField", &GET_BOOLEAN_FIELD),
    ("JNIEnv.GetIntField", &GET_INT_FIELD),
    ("JNIEnv.SetObjectField", &SET_OBJ_FIELD),
    ("JNIEnv.SetBooleanField", &SET_BOOLEAN_FIELD),
    ("JNIEnv.GetStaticMethodID", &GET_STATIC_METHOD_ID),
    ("JNIEnv.NewStringUTF", &NEW_STRING_UTF),
    ("JNIEnv.GetStringUTFLength", &GET_STRING_UTF_LEN),
    ("JNIEnv.GetStringUTFChars", &GET_STRING_UTF_CHARS),
    ("JNIEnv.GetArrayLength", &GET_ARRAY_LEN),
    ("JNIEnv.NewObjectArray", &NEW_OBJECT_ARRAY),
    ("JNIEnv.GetObjectArrayElement", &GET_OBJ_ARR_ELEM),
    ("JNIEnv.SetObjectArrayElement", &SET_OBJ_ARR_ELEM),
    ("JNIEnv.NewByteArray", &NEW_BYTE_ARRAY),
    ("JNIEnv.NewIntArray", &NEW_INT_ARRAY),
    ("JNIEnv.GetByteArrayElements", &GET_BYTE_ARRAY_ELEMENTS),
    ("JNIEnv.GetIntArrayElements", &GET_INT_ARRAY_ELEMENTS),
    ("JNIEnv.ReleaseByteArrayElements", &RELEASE_BYTE_ARRAY_ELEMENTS),
    ("JNIEnv.ReleaseIntArrayElements", &RELEASE_INT_ARRAY_ELEMENTS),
    ("JNIEnv.GetByteArrayRegion", &GET_BYTE_ARRAY_REGION),
    ("JNIEnv.GetIntArrayRegion", &GET_INT_ARRAY_REGION),
    ("JNIEnv.SetByteArrayRegion", &SET_BYTE_ARRAY_REGION),
    ("JNIEnv.SetIntArrayRegion", &SET_INT_ARRAY_REGION),
    ("JNIEnv.RegisterNatives", &REGISTER_NATIVES),
    ("JNIEnv.GetJavaVM", &GET_JAVA_VM),
    ("JNIEnv.IsAssignableFrom", &IS_ASSIGNABLE_FROM),
    ("JNIEnv.ExceptionOccurred", &EXCEPTION_OCCURRED),
    ("JNIEnv.ExceptionDescribe", &EXCEPTION_DESCRIBE),
    ("JNIEnv.ExceptionClear", &EXCEPTION_CLEAR),
    ("JNIEnv.PushLocalFrame", &PUSH_LOCAL_FRAME),
    ("JNIEnv.PopLocalFrame", &POP_LOCAL_FRAME),
    ("JNIEnv.IsSameObject", &IS_SAME_OBJECT),
    ("JNIEnv.NewLocalRef", &NEW_LOCAL_REF),
    ("JNIEnv.EnsureLocalCapacity", &ENSURE_LOCAL_CAPACITY),
    ("JNIEnv.AllocObject", &ALLOC_OBJECT),
    ("JNIEnv.NewObject", &NEW_OBJECT),
    ("JNIEnv.GetObjectClass", &GET_OBJECT_CLASS),
    ("JNIEnv.IsInstanceOf", &IS_INSTANCE_OF),
    ("JNIEnv.CallObjectMethod", &CALL_OBJECT_METHOD),
    ("JNIEnv.CallBooleanMethod", &CALL_BOOLEAN_METHOD),
    ("JNIEnv.CallIntMethod", &CALL_INT_METHOD),
    ("JNIEnv.CallLongMethod", &CALL_LONG_METHOD),
    ("JNIEnv.CallFloatMethod", &CALL_FLOAT_METHOD),
    ("JNIEnv.CallVoidMethod", &CALL_VOID_METHOD),
    ("JNIEnv.CallStaticObjectMethod", &CALL_STATIC_OBJECT_METHOD),
    ("JNIEnv.CallStaticBooleanMethod", &CALL_STATIC_BOOLEAN_METHOD),
    ("JNIEnv.CallStaticIntMethod", &CALL_STATIC_INT_METHOD),
    ("JNIEnv.CallStaticVoidMethod", &CALL_STATIC_VOID_METHOD),
    ("JNIEnv.GetStaticFieldID", &GET_STATIC_FIELD_ID),
    ("JNIEnv.GetStaticObjectField", &GET_STATIC_OBJECT_FIELD),
    ("JNIEnv.GetStaticIntField", &GET_STATIC_INT_FIELD),
    ("JNIEnv.SetStaticObjectField", &SET_STATIC_OBJECT_FIELD),
    ("JNIEnv.SetStaticIntField", &SET_STATIC_INT_FIELD),
    ("JNIEnv.ReleaseStringUTFChars", &RELEASE_STRING_UTF_CHARS),
    ("JNIEnv.UnregisterNatives", &UNREGISTER_NATIVES),
    ("JNIEnv.MonitorEnter", &MONITOR_ENTER),
    ("JNIEnv.MonitorExit", &MONITOR_EXIT),
    ("JNIEnv.GetStringUTFRegion", &GET_STRING_UTF_REGION),
    ("JNIEnv.NewWeakGlobalRef", &NEW_WEAK_GLOBAL_REF),
    ("JNIEnv.DeleteWeakGlobalRef", &DELETE_WEAK_GLOBAL_REF),
    ("JNIEnv.ExceptionCheck", &EXCEPTION_CHECK),
    ("JNIEnv.NewDirectByteBuffer", &NEW_DIRECT_BYTE_BUFFER),
    ("JNIEnv.GetDirectBufferAddress", &GET_DIRECT_BUFFER_ADDRESS),
    ("JNIEnv.GetDirectBufferCapacity", &GET_DIRECT_BUFFER_CAPACITY),
];

/// Registry of interned UTF-8 byte strings: `str_handle` allocates a readable,
/// null-terminated copy in guest-addressable memory (guest==host) and returns a
/// stable handle for the same bytes. This is the JIT analogue of the QEMU
/// shim's `track_ptr`: a *readable* buffer rather than a low sentinel like
/// 0x3000, so guest code that dereferences a returned jclass/jstring/methodID
/// reads valid memory instead of faulting.
fn str_handle(bytes: &[u8]) -> u64 {
    static REG: OnceLock<Mutex<HashMap<Vec<u8>, u64>>> = OnceLock::new();
    let mut reg = REG.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    if let Some(&h) = reg.get(bytes) {
        return h;
    }
    let layout = Layout::array::<u8>(bytes.len() + 1).unwrap();
    let p = unsafe { alloc_zeroed(layout) } as *mut u8;
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    let addr = p as u64;
    reg.insert(bytes.to_vec(), addr);
    addr
}

/// Guest-visible registry of native methods registered via `RegisterNatives`.
///
/// Android's `JNI_OnLoad` calls `env->RegisterNatives(clazz, methods, nMethods)`
/// where `methods` is a `JNINativeMethod` array (3 u64 words each: `name`,
/// `signature`, `fnPtr`). The JIT's `jni_register_natives` host stub previously
/// returned JNI_OK and DROPPED the array — so a native method Roblox binds
/// (e.g. `Java_com_roblox_..._IAP...`) could never be found again when the host
/// runtime later dispatches back into the guest. This registry parses the
/// guest array (guest==host, so a host read is a guest read) and records
/// `(class, name) -> (signature, fnPtr)` where `fnPtr` is the *guest* address
/// of the Java_* implementation, ready to be driven through `jit_run` as a
/// guest entry point.
#[derive(Clone, Debug)]
pub struct NativeMethod {
    pub class: Vec<u8>,
    pub name: Vec<u8>,
    pub signature: Vec<u8>,
    pub fn_ptr: u64,
}

fn native_registry() -> &'static Mutex<Vec<NativeMethod>> {
    static REG: OnceLock<Mutex<Vec<NativeMethod>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Vec::new()))
}

/// Wipe the registry (test-only / re-boot hygiene; also guarded so a fresh
/// JNI_OnLoad doesn't accumulate stale bindings with side effects).
pub fn clear_native_methods() {
    native_registry().lock().unwrap().clear();
}

/// Look up a registered native method by (class, name); returns the guest
/// `fnPtr` and signature. This is what a host dispatch of a Roblox Java_*
/// method needs to convert a Java method name into a runnable guest entry.
pub fn lookup_native_method(class: &[u8], name: &[u8]) -> Option<NativeMethod> {
    let reg = native_registry().lock().unwrap();
    // Last-registration wins (JNI semantics: re-registering replaces).
    reg.iter().rev().find(|m| m.class == class && m.name == name).cloned()
}

/// Record a native method binding from the guest `JNINativeMethod` array at
/// `methods` (nMethods words of sizt 3). Parse each entry's three u64 words
/// (name*, signature*, fnPtr). A missing/unreadable name or a NULL fnPtr is a
/// malformed register call; skip it (JNI treats fnPtr==NULL as "delete
/// binding" — stricter handling can come later) rather than fault.
fn parse_register_natives(cls: u64, methods: u64, n: u64) {
    if methods == 0 {
        return;
    }
    let class_bytes = read_cstr(cls).unwrap_or_default();
    let mut reg = native_registry().lock().unwrap();
    let base = methods as *const u64;
    for i in 0..n {
        let e = unsafe { base.add(i as usize * 3) };
        let name_p = unsafe { *e };
        let sig_p = unsafe { *e.add(1) };
        let fn_ptr = unsafe { *e.add(2) };
        let Some(name) = read_cstr(name_p) else { continue };
        let sig = read_cstr(sig_p).unwrap_or_default();
        if std::env::var_os("JNI_TRACE_REGISTRY").is_some() {
            eprintln!("[jni] RegisterNatives {}::{} {} -> {:#x}", String::from_utf8_lossy(&class_bytes), String::from_utf8_lossy(&name), String::from_utf8_lossy(&sig), fn_ptr);
        }
        reg.push(NativeMethod { class: class_bytes.clone(), name, signature: sig, fn_ptr });
    }
}

/// Read a NUL-terminated C string from guest memory (`guest==host`, so a host
/// read is a guest read). Bounded to avoid over-reading a bad pointer.
fn read_cstr(p: u64) -> Option<Vec<u8>> {
    if p == 0 {
        return None;
    }
    const MAX: usize = 4096;
    let p = p as *const u8;
    let mut v = Vec::new();
    for i in 0..MAX {
        let c = unsafe { *p.add(i) };
        if c == 0 {
            return Some(v);
        }
        v.push(c);
    }
    None
}

/// Read the C string at `ptr` and intern it via `str_handle` (0 if unreadable).
fn cstr_handle(ptr: u64) -> u64 {
    read_cstr(ptr).map_or(0, |s| str_handle(&s))
}

/// Generic "return a constant / ignore args" stub.
macro_rules! jni_stub {
    ($name:ident, $val:expr) => {
        extern "C" fn $name(
            _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
        ) -> u64 {
            $val
        }
    };
}

/// Guest-visible registry of primitive-array handles -> byte length. A jbyteArray /
/// jintArray handle is a readable/writable guest-addressable buffer; this registry
/// records how many bytes it holds so Get/Set*Region can bounds-check and copy.
/// Mirrors the QEMU shim's model where a primitive array is just a host buffer.
fn array_len_registry() -> &'static Mutex<HashMap<u64, usize>> {
    static REG: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Allocate a zeroed primitive array of `elem_size`-byte elements and return its
/// guest-addressable handle (the buffer pointer). Records its byte length.
pub fn jni_new_array_raw(len: usize, elem_size: usize) -> u64 {
    let bytes = len.checked_mul(elem_size).unwrap_or(0);
    let layout = Layout::array::<u8>(bytes.max(1)).unwrap();
    let p = unsafe { alloc_zeroed(layout) } as *mut u8;
    let addr = p as u64;
    array_len_registry().lock().unwrap().insert(addr, bytes);
    addr
}

extern "C" fn jni_new_byte_array(
    _e: u64, len: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    jni_new_array_raw(len as usize, 1)
}

extern "C" fn jni_new_int_array(
    _e: u64, len: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    jni_new_array_raw(len as usize, 4)
}

/// jint GetArrayLength(JNIEnv*, jarray): object arrays are backed the same way as
/// primitive arrays (a guest-addressable buffer + byte length), so this shared
/// accessor works for them too. See note below about byte-length reporting.
extern "C" fn jni_get_array_length(
    _e: u64, arr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let reg = array_len_registry().lock().unwrap();
    match reg.get(&arr) {
        // jintArray len is bytes/4; jbyteArray len is bytes/1. We track both by
        // storing byte length and reconstructing element count is ambiguous for
        // Set*Region, so callers pass element count explicitly there. For
        // GetArrayLength the guest uses it on byte arrays mostly; report bytes.
        Some(&b) => b as u64,
        None => 0,
    }
}

/// NewObjectArray(jint length, jclass elementClass, jobject initialElement):
/// allocate a backing buffer of `length` pointer-sized jobject slots, optionally
/// seeding each with `init` (usually NULL). Every slot holds an opaque
/// guest-addressable handle, so an object array is just an 8-byte-element array.
extern "C" fn jni_new_object_array(
    _e: u64, len: u64, _elem_class: u64, init: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let addr = jni_new_array_raw(len as usize, 8);
    if addr != 0 && init != 0 {
        let n = len as usize;
        unsafe {
            for i in 0..n {
                *((addr as *mut u64).add(i)) = init;
            }
        }
    }
    addr
}

/// GetObjectArrayElement(JNIEnv*, jobjectArray array, jsize index) -> jobject:
/// read the opaque handle stored at `array[index]` (bounds-checked). Returns
/// NULL on an out-of-range index (JNI semantics) rather than faulting.
extern "C" fn jni_get_object_array_element(
    _e: u64, arr: u64, index: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let total = array_len_registry().lock().unwrap().get(&arr).copied().unwrap_or(0);
    if total == 0 || arr == 0 {
        return 0;
    }
    let idx = index as usize;
    let elems = total / 8;
    if idx >= elems {
        return 0; // out of range -> NULL (JNI getObjectArrayElement returns NULL)
    }
    unsafe { *((arr as *const u64).add(idx)) }
}

/// SetObjectArrayElement(JNIEnv*, jobjectArray array, jsize index, jobject val):
/// write the opaque handle `val` into `array[index]` (bounds-checked). No-op
/// (returning void) on an out-of-range index.
extern "C" fn jni_set_object_array_element(
    _e: u64, arr: u64, index: u64, val: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let total = array_len_registry().lock().unwrap().get(&arr).copied().unwrap_or(0);
    if total == 0 || arr == 0 {
        return 0;
    }
    let idx = index as usize;
    let elems = total / 8;
    if idx >= elems {
        return 0;
    }
    unsafe { *((arr as *mut u64).add(idx)) = val; }
    0
}

/// Get<Primitive>ArrayElements: return the buffer pointer; `*isCopy` = 0 (no copy).
extern "C" fn jni_get_byte_array_elements(
    _e: u64, arr: u64, iscopy: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if iscopy != 0 {
        unsafe { *(iscopy as *mut i8) = 0 };
    }
    arr
}

extern "C" fn jni_get_int_array_elements(
    _e: u64, arr: u64, iscopy: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if iscopy != 0 {
        unsafe { *(iscopy as *mut i8) = 0 };
    }
    arr
}

/// Release<Primitive>ArrayElements: we hand out the backing buffer directly, so
/// nothing to free; mode 0 (ABORT) / JNI_COMMIT (1) / JNI_ABORT (2) all no-op.
extern "C" fn jni_release_byte_array_elements(
    _e: u64, _arr: u64, _elems: u64, _mode: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    0
}

extern "C" fn jni_release_int_array_elements(
    _e: u64, _arr: u64, _elems: u64, _mode: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    0
}

/// Copy `len` bytes from `src` starting at `start` into the array's backing buffer.
extern "C" fn jni_set_byte_array_region(
    _e: u64, arr: u64, start: u64, len: u64, src: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let total = array_len_registry().lock().unwrap().get(&arr).copied();
    let Some(total) = total else {
        return 0;
    };
    let start = start as usize;
    let len = len as usize;
    let end = start.saturating_add(len);
    if end > total || src == 0 || arr == 0 {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(src as *const u8, arr as *mut u8, len);
    }
    0
}

/// Copy `len` bytes from the array's backing buffer into `dst`.
extern "C" fn jni_get_byte_array_region(
    _e: u64, arr: u64, start: u64, len: u64, dst: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let total = array_len_registry().lock().unwrap().get(&arr).copied();
    let Some(total) = total else {
        return 0;
    };
    let start = start as usize;
    let len = len as usize;
    let end = start.saturating_add(len);
    if end > total || dst == 0 || arr == 0 {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(arr as *const u8, dst as *mut u8, len);
    }
    0
}

/// SetIntArrayRegion: copy `len` 32-bit ints from `src` into `arr[start..start+len)`.
extern "C" fn jni_set_int_array_region(
    _e: u64, arr: u64, start: u64, len: u64, src: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if src == 0 || arr == 0 {
        return 0;
    }
    let len = (len as usize).saturating_mul(4);
    unsafe {
        std::ptr::copy_nonoverlapping(src as *const u8, arr as *mut u8, len);
    }
    0
}

/// GetIntArrayRegion: copy `len` 32-bit ints from `arr[start..]` into `dst`.
extern "C" fn jni_get_int_array_region(
    _e: u64, arr: u64, start: u64, len: u64, dst: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if dst == 0 || arr == 0 {
        return 0;
    }
    let len = (len as usize).saturating_mul(4);
    unsafe {
        std::ptr::copy_nonoverlapping(arr as *const u8, dst as *mut u8, len);
    }
    0
}

/// Register an array handle + byte length (used by boot glue that constructs
/// Guest-visible arrays); test/helper surface.
pub fn register_array(addr: u64, bytes: usize) {
    array_len_registry().lock().unwrap().insert(addr, bytes);
}

jni_stub!(jni_get_version, JNI_VERSION_1_6 as u64);
jni_stub!(jni_voidp_0, 0); // default: return NULL/0
jni_stub!(jni_ok, 0); // JNI_OK / status-returning stubs
jni_stub!(jni_field_0, 0);
 
extern "C" fn jni_find_class(
    _e: u64, name: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let h = cstr_handle(name); // jclass = readable, stable handle
    if std::env::var_os("JIT_TRACE").is_some() {
        eprintln!("[jni] FindClass({name:#x}) -> {h:#x}");
    }
    h
}

extern "C" fn jni_get_method_id(
    _e: u64, _cls: u64, name: u64, sig: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    // Readable stable handle (method IDs are opaque to guests; safety over sentinel).
    let _ = sig;
    cstr_handle(name)
}

extern "C" fn jni_new_string_utf(
    _e: u64, utf: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let h = cstr_handle(utf); // jstring = readable UTF-8 buffer
    if std::env::var_os("JIT_TRACE").is_some() {
        eprintln!("[jni] NewStringUTF({utf:#x}) -> {h:#x}");
    }
    h
}

extern "C" fn jni_get_string_utf_chars(
    _e: u64, jstr: u64, iscopy: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if iscopy != 0 {
        unsafe { *(iscopy as *mut i8) = 0 }; // "no copy returned"
    }
    jstr // the jstring IS the readable buffer we gave out in NewStringUTF
}

extern "C" fn jni_get_string_utf_length(
    _e: u64, jstr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    read_cstr(jstr).map_or(0, |s| s.len() as u64)
}

// ---------------------------------------------------------------------------
// AutoValue AppBridge params getter shim.
//
// The engine's V2 boot ladder (nativeGameGlobalInit -> setTaskSchedulerBM ->
// nativeAppBridgeV2InitWithParams -> nativeAppBridgeStartLuaAppDM ->
// nativeAppBridgeV2StartAppWithParams) reads its InitParams/StartAppParams/
// DeviceParams as Java AutoValue objects: it does GetMethodID(cls,"getBaseURL",
// ...) then CallObjectMethod(params, methodID, ...) to pull each field, and
// serializes them into the json it sends the engine. Our JNI GetMethodID
// returns a READABLE handle of the METHOD NAME, so a Call*Method stub can
// dispatch on the getter name and return a REAL value. Before this shim every
// Call*Method returned 0, so StartApp's serialization of the params read
// UNINITIALIZED guest-stack std::strings -> the SH45/SH46 RBX::json
// string-length-overflow abort. Treating the getters as AutoValue accessors
// and returning valid empty/default strings gives the json writer a valid
// length (0) instead of a stack pointer. Unrecognized method names still
// return 0 (the honest fallback), so unrelated Call*Method sites are unchanged.
fn method_id_name(mid: u64) -> Option<Vec<u8>> {
    read_cstr(mid)
}

/// The AppBridge params' string-valued AutoValue getters -> their default.
/// Most default to empty; the client expects selectedTheme="Dark". Returning
/// "" (a real, readable, zero-length jstring) rather than NULL lets the json
/// writer read a valid 0 length instead of dereferencing NULL or an
/// uninitialized stack slot.
fn auto_value_string_getter(name: &[u8]) -> Option<&'static [u8]> {
    match name {
        b"getBaseURL" => Some(b""),
        b"getBuildVariant" => Some(b""),
        b"getUserAgent" => Some(b""),
        b"getAppStarterPlace" => Some(b""),
        b"getAppStarterScript" => Some(b""),
        b"getSelectedTheme" => Some(b"Dark"),
        b"getUsername" => Some(b""),
        b"getAppUserId" => Some(b""),
        b"getDeviceParams" => Some(b""),
        b"getPlatformParams" => Some(b""),
        b"getVrContext" => Some(b""),
        b"getSurface" => Some(b""),
        // DeviceParams (recon v2 shape). osVersion is the Vulkan GATE: below
        // "33" the engine refuses to initialize the Vulkan renderer, so a real
        // value here is load-bearing, not cosmetic.
        b"getOsVersion" => Some(b"33"),
        b"getDeviceName" => Some(b"Cordial"),
        b"getDeviceSku" => Some(b"cordial"),
        b"getManufacturer" => Some(b"Cordial"),
        b"getCountry" => Some(b"US"),
        b"getNetworkType" => Some(b"WIFI"),
        b"getAppVersion" => Some(b""),
        _ => None,
    }
}

/// CallObjectMethod(env, obj, methodID, ...): return a real jstring handle for
/// the AppBridge params string getters; 0 for anything else (unchanged legacy).
extern "C" fn jni_call_object_method(
    _e: u64, _obj: u64, mid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if let Some(name) = method_id_name(mid) {
        if let Some(val) = auto_value_string_getter(&name) {
            if std::env::var_os("JIT_TRACE").is_some() {
                eprintln!("[jni] CallObjectMethod getter {} -> {}B string handle", String::from_utf8_lossy(&name), val.len());
            }
            return str_handle(val);
        }
    }
    0
}

/// CallBooleanMethod(env, obj, methodID, ...): the AppBridge params boolean
/// getters. Defaults per recon v2 — isUnder13/isPotato/isTablet/isVrDevice/
/// isTouchDevice/isLowRamDevice false, isKeyboardDevice/isMouseDevice/
/// isCpu64Bit true. Unrecognized -> 0 (false).
extern "C" fn jni_call_boolean_method(
    _e: u64, _obj: u64, mid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    match method_id_name(mid).as_deref() {
        Some(b"isUnder13") => 0,        // default false
        Some(b"isPotato") => 0,
        Some(b"isTablet") => 0,
        Some(b"isVrDevice") => 0,
        Some(b"isTouchDevice") => 0,
        Some(b"isLowRamDevice") => 0,
        Some(b"isKeyboardDevice") => 1, // true (desktop has a keyboard)
        Some(b"isMouseDevice") => 1,
        Some(b"isCpu64Bit") => 1,       // true (we are 64-bit)
        _ => 0,
    }
}

/// CallIntMethod(env, obj, methodID, ...): the AppBridge params integer
/// getters. getMembershipType default 0. Unrecognized -> 0.
extern "C" fn jni_call_int_method(
    _e: u64, _obj: u64, mid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    match method_id_name(mid).as_deref() {
        Some(b"getMembershipType") => 0,
        _ => 0,
    }
}

/// CallLongMethod(env, obj, methodID, ...): the AppBridge params long getters.
/// getAppUserId defaults 0; deviceMemoryMB is 8192 (recon v2). Unrecognized -> 0.
extern "C" fn jni_call_long_method(
    _e: u64, _obj: u64, mid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    match method_id_name(mid).as_deref() {
        Some(b"getAppUserId") => 0,
        Some(b"getDeviceTotalMemoryMB") => 8192,
        // LocalStorageManager.getAllocatableBytes() (recon v2): the REAL free
        // space. Returning 0 makes the engine believe there is no disk and
        // RbxStorage never builds its content cache — a remembered session's
        // cache-plane precondition. Report the host filesystem's actual free
        // bytes for the persistence root so cache sizing matches reality.
        Some(b"getAllocatableBytes") => allocatable_bytes(),
        _ => 0,
    }
}

/// Host free-bytes under the armed SOBER_ANDROID_ROOT (real disk space the
/// engine's LocalStorageManager cache sizing should see). Falls back to 0 only
/// if even the root dir cannot be stat'ed (disk unknown).
fn allocatable_bytes() -> u64 {
    let root = crate::fsmap::configured_root();
    let probe = root
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/"));
    let file = match std::fs::File::open(&probe) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    match nix_sys_stbufs::statvfs_of(&file) {
        Some((bsize, bavail)) => bsize.saturating_mul(bavail),
        None => 0,
    }
}

// Tiny shim: return the filesystem's block size + free blocks for an open file
// via libc::fstatvfs (avail * bsize = allocatable bytes).
mod nix_sys_stbufs {
    pub fn statvfs_of(f: &std::fs::File) -> Option<(u64, u64)> {
        use std::os::unix::io::AsRawFd;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::fstatvfs(f.as_raw_fd(), &mut st) };
        if rc != 0 {
            return None;
        }
        Some((st.f_frsize as u64, st.f_bavail as u64))
    }
}

/// CallFloatMethod(env, obj, methodID, ...) — `jfloat` getters. Returns a
/// `u32` bit-pattern that the dispatcher writes into guest s0 (the AAPCS64
/// float return register), so the layout gate getDpiScale yields 1.0 rather
/// than a collapsed 0. Registered as a `HostJniF32` bridge (whole-CpuState,
/// s0-return); the args (env/obj/mid) are read from the x-registers in state.
extern "C" fn jni_call_float_method(state: *mut crate::jit::CpuState) -> u32 {
    let st = unsafe { &*state };
    let mid = st.x[2];
    match method_id_name(mid).as_deref() {
        Some(b"getDpiScale") => 1.0f32.to_bits(), // layout gate; read 3x
        _ => 0,
    }
}

extern "C" fn jni_register_natives(
    _e: u64, cls: u64, methods: u64, n: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    // Parse the guest JNINativeMethod array and record (class,name,sig)->fnPtr
    // so a registered Roblox Java_* method can later be dispatched back into
    // the guest as a jit_run entry. (See parse_register_natives docs.)
    parse_register_natives(cls, methods, n);
    0 // JNI_OK
}

extern "C" fn jni_new_global_ref(
    _e: u64, o: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    o // pass-through like the QEMU shim
}

/// NewLocalRef: identity (the fake-object model has no distinct local-ref pool,
/// so a returned ref is just the object's handle — like NewGlobalRef). Non-zero
/// for a valid object: guest `if (!local) return` doesn't spuriously abort.
extern "C" fn jni_new_local_ref(
    _e: u64, o: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    o
}

/// IsSameObject(a, b): 1 iff the two handles are identical. Fake-object handles
/// are invariant tokens, so identity comparison is the correct semantic.
extern "C" fn jni_is_same_object(
    _e: u64, a: u64, b: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    (a == b) as u64
}

/// GetObjectClass(obj): return a stable non-zero jclass handle so guest code that
/// does `jclass c = GetObjectClass(o); if (!c) fail;` passes. Never return `o`
/// itself (that would alias the object). Interning a fixed class name yields a
/// readable, stable handle that won't collide with real object addresses.
extern "C" fn jni_get_object_class(
    _e: u64, obj: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if obj == 0 {
        0
    } else {
        str_handle(b"java/lang/Object")
    }
}

/// IsInstanceOf(obj, cls): permissive true (1). Under fake-object backing any
/// object satisfies any tested class, so instanceof guards take the success
/// branch instead of a NULL/abort path.
extern "C" fn jni_is_instance_of(
    _e: u64, _obj: u64, _cls: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    1
}

/// GetStringUTFRegion(env, str, start, len, buf): copy `len` UTF-8 bytes of the
/// string starting at byte `start` into `buf` (bounds-safe). `str` is the
/// readable buffer NewStringUTF returned.
extern "C" fn jni_get_string_utf_region(
    _e: u64, jstr: u64, start: u64, len: u64, buf: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if buf == 0 {
        return 0;
    }
    let s = read_cstr(jstr).unwrap_or_default();
    let start = start as usize;
    if start > s.len() {
        return 0;
    }
    let n = (len as usize).min(s.len() - start);
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr().add(start), buf as *mut u8, n);
    }
    0
}

/// Guest-visible registry of direct NIO byte buffers: NewDirectByteBuffer's
/// returned jobject handle -> (native address, capacity). Roblox passes
/// textures/audio/asset buffers around as `java.nio.ByteBuffer` native memory,
/// so GetDirectBufferAddress/Capacity must answer with the real backing.
fn direct_buffer_registry() -> &'static Mutex<HashMap<u64, (u64, u64)>> {
    static REG: OnceLock<Mutex<HashMap<u64, (u64, u64)>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// NewDirectByteBuffer(env, address, capacity) -> jobject: create a fresh,
/// unique opaque handle that records (address, capacity). Returns the handle
/// (non-zero) so `ByteBuffer.allocateDirect`-style checks don't abort; the
/// address/capacity are recovered by GetDirectBufferAddress/Capacity.
extern "C" fn jni_new_direct_byte_buffer(
    _e: u64, address: u64, capacity: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    if address == 0 {
        return 0;
    }
    let token = unsafe { alloc_zeroed(Layout::new::<u8>()) } as u64; // fresh unique handle
    direct_buffer_registry().lock().unwrap().insert(token, (address, capacity));
    token
}

extern "C" fn jni_get_direct_buffer_address(
    _e: u64, buf: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    direct_buffer_registry().lock().unwrap().get(&buf).map(|&(a, _)| a).unwrap_or(0)
}

extern "C" fn jni_get_direct_buffer_capacity(
    _e: u64, buf: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    direct_buffer_registry().lock().unwrap().get(&buf).map(|&(_, c)| c).unwrap_or(0)
}

/// IsAssignableFrom: permissive true (1) — fake-object classes always assignable.
extern "C" fn jni_self_true(
    _e: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    1
}

/// PopLocalFrame(env, result): returns its `result` argument (JNI semantics — the
/// popped frame's saved result object, or NULL).
extern "C" fn jni_pop_local_frame(
    _e: u64, result: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    result
}

extern "C" fn jni_get_java_vm(
    _e: u64, vm_out: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let (_env, vm) = build_jni();
    if vm_out != 0 {
        unsafe { *(vm_out as *mut u64) = vm };
    }
    0
}

// JVM `GetEnv`/`AttachCurrentThread`: `jint GetEnv(JavaVM*, void** penv, jint)`
// writes the current thread's JNIEnv into `*penv` and returns JNI_OK (0).
extern "C" fn jni_vm_getenv(
    vm: u64, penv: u64, _version: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64, _a7: u64,
) -> u64 {
    let (env, vm2) = build_jni();
    if std::env::var_os("JIT_TRACE").is_some() {
        eprintln!("[jni] VM_GetEnv(vm={vm:#x} penv={penv:#x}) writes env={env:#x} [env]={:#x} vm2={vm2:#x}", unsafe { *(env as *const u64) });
    }
    if penv != 0 {
        unsafe { *(penv as *mut u64) = env };
    }
    0 // JNI_OK
}

/// Build the singleton guest-visible JNIEnv and JavaVM, each with a pointer
/// table whose slots are host-thunk guest addresses. Returns `(jni_env, java_vm)`.
pub fn build_jni() -> (u64, u64) {
    static ONE: OnceLock<(u64, u64)> = OnceLock::new();
    *ONE.get_or_init(|| {
        let version = reg(jni_get_version);
        let voidp = reg(jni_voidp_0);
        let ok = reg(jni_ok);
        let field0 = reg(jni_field_0);

        // Default every slot to a NULL/0-returning stub so an unserviced slot
        // never dispatches to NULL.
        let mut functions: Vec<u64> = vec![voidp; JNI_SLOTS];
        functions[GET_VERSION] = version;
        functions[FIND_CLASS] = reg(jni_find_class);
        functions[THROW] = ok;
        functions[THROW_NEW] = ok;
        functions[NEW_GLOBAL_REF] = reg(jni_new_global_ref);
        functions[DEL_GLOBAL_REF] = ok;
        functions[DEL_LOCAL_REF] = ok;
        functions[GET_METHOD_ID] = reg(jni_get_method_id);
        functions[GET_FIELD_ID] = reg(jni_get_method_id);
        functions[GET_OBJ_FIELD] = field0;
        functions[GET_BOOLEAN_FIELD] = field0;
        functions[GET_INT_FIELD] = field0;
        functions[SET_OBJ_FIELD] = ok;
        functions[SET_BOOLEAN_FIELD] = ok;
        functions[GET_STATIC_METHOD_ID] = reg(jni_get_method_id);
        functions[NEW_STRING_UTF] = reg(jni_new_string_utf);
        functions[GET_STRING_UTF_LEN] = reg(jni_get_string_utf_length);
        functions[GET_STRING_UTF_CHARS] = reg(jni_get_string_utf_chars);
        functions[GET_ARRAY_LEN] = reg(jni_get_array_length);
        functions[NEW_OBJECT_ARRAY] = reg(jni_new_object_array);
        functions[GET_OBJ_ARR_ELEM] = reg(jni_get_object_array_element);
        functions[SET_OBJ_ARR_ELEM] = reg(jni_set_object_array_element);
        // Primitive-array creation + accessors (authoritative NDK offsets). These
        // previously fell to the NULL/voidp default, so Roblox texture/file/GL-buffer
        // jbyteArray/jintArray work returned garbage or crashed.
        functions[NEW_BYTE_ARRAY] = reg(jni_new_byte_array);
        functions[NEW_INT_ARRAY] = reg(jni_new_int_array);
        functions[GET_BYTE_ARRAY_ELEMENTS] = reg(jni_get_byte_array_elements);
        functions[GET_INT_ARRAY_ELEMENTS] = reg(jni_get_int_array_elements);
        functions[RELEASE_BYTE_ARRAY_ELEMENTS] = reg(jni_release_byte_array_elements);
        functions[RELEASE_INT_ARRAY_ELEMENTS] = reg(jni_release_int_array_elements);
        functions[GET_BYTE_ARRAY_REGION] = reg(jni_get_byte_array_region);
        functions[GET_INT_ARRAY_REGION] = reg(jni_get_int_array_region);
        functions[SET_BYTE_ARRAY_REGION] = reg(jni_set_byte_array_region);
        functions[SET_INT_ARRAY_REGION] = reg(jni_set_int_array_region);
        functions[REGISTER_NATIVES] = reg(jni_register_natives);
        functions[GET_JAVA_VM] = reg(jni_get_java_vm);
        // Fake-object backing + the rest of the JNI surface at official offsets.
        // Exception / local-frame / capacity / monitor / void / zero-returning
        // call+field slots use the shared NULL-returning stubs; object-model,
        // ref, direct-buffer and string-copy slots get real fake-object backing.
        functions[IS_ASSIGNABLE_FROM] = reg(jni_self_true); // permissive
        functions[EXCEPTION_OCCURRED] = reg(jni_voidp_0); // no pending exception
        functions[EXCEPTION_DESCRIBE] = ok;
        functions[EXCEPTION_CLEAR] = ok;
        functions[PUSH_LOCAL_FRAME] = ok; // JNI_OK
        functions[POP_LOCAL_FRAME] = reg(jni_pop_local_frame);
        functions[IS_SAME_OBJECT] = reg(jni_is_same_object);
        functions[NEW_LOCAL_REF] = reg(jni_new_local_ref);
        functions[ENSURE_LOCAL_CAPACITY] = ok; // JNI_OK
        functions[ALLOC_OBJECT] = reg(jni_voidp_0); // no java.lang.Object to alloc
        functions[NEW_OBJECT] = reg(jni_voidp_0);
        functions[GET_OBJECT_CLASS] = reg(jni_get_object_class);
        functions[IS_INSTANCE_OF] = reg(jni_is_instance_of);
        // Call[Object/Boolean/Int/Void]Method + the static forms: return the
        // typed zero. Real Java re-entry isn't wired, so 0 is the honest result.
        // The AutoValue AppBridge params getters are serviced by the getter
        // shim (jni_call_object_method etc.) so StartApp's serialization reads
        // valid empty/default strings instead of uninitialized guest-stack
        // std::strings (the SH45/SH46 json string-length-overflow root cause).
        functions[CALL_OBJECT_METHOD] = reg(jni_call_object_method);
        functions[CALL_BOOLEAN_METHOD] = reg(jni_call_boolean_method);
        functions[CALL_INT_METHOD] = reg(jni_call_int_method);
        functions[CALL_LONG_METHOD] = reg(jni_call_long_method);
        functions[CALL_FLOAT_METHOD] = reg_jni_f32(jni_call_float_method);
        functions[CALL_VOID_METHOD] = ok;
        functions[CALL_STATIC_OBJECT_METHOD] = reg(jni_voidp_0);
        functions[CALL_STATIC_BOOLEAN_METHOD] = reg(jni_voidp_0);
        functions[CALL_STATIC_INT_METHOD] = reg(jni_voidp_0);
        functions[CALL_STATIC_VOID_METHOD] = ok;
        functions[GET_STATIC_FIELD_ID] = reg(jni_get_method_id);
        functions[GET_STATIC_OBJECT_FIELD] = reg(jni_voidp_0);
        functions[GET_STATIC_INT_FIELD] = field0;
        functions[SET_STATIC_OBJECT_FIELD] = ok;
        functions[SET_STATIC_INT_FIELD] = ok;
        functions[RELEASE_STRING_UTF_CHARS] = ok; // void no-op
        functions[UNREGISTER_NATIVES] = ok; // JNI_OK
        functions[MONITOR_ENTER] = ok; // JNI_OK
        functions[MONITOR_EXIT] = ok; // JNI_OK
        functions[GET_STRING_UTF_REGION] = reg(jni_get_string_utf_region);
        functions[NEW_WEAK_GLOBAL_REF] = reg(jni_new_global_ref); // identity
        functions[DELETE_WEAK_GLOBAL_REF] = ok;
        functions[EXCEPTION_CHECK] = reg(jni_voidp_0); // no pending exception
        functions[NEW_DIRECT_BYTE_BUFFER] = reg(jni_new_direct_byte_buffer);
        functions[GET_DIRECT_BUFFER_ADDRESS] = reg(jni_get_direct_buffer_address);
        functions[GET_DIRECT_BUFFER_CAPACITY] = reg(jni_get_direct_buffer_capacity);

        let env_fn_tbl = u64array(&functions);
        let mut vm_functions = vec![voidp; VM_SLOTS];
        let vm_getenv = reg(jni_vm_getenv);
        vm_functions[VM_GET_ENV] = vm_getenv; // GetEnv: writes *penv=env, returns JNI_OK
        let vm_fn_tbl = u64array(&vm_functions);

        // Record a human name for every filled JNIEnv/JavaVM slot so the JIT_TRACE
        // hostcall dumper prints which JNI function the engine dispatches (instead
        // of an anonymous `slotN`). This is what makes the real-boot run-log
        // readable when GameActivity init churns JNI calls.
        for (func_name, slot) in JNI_METHOD_NAMES {
            let idx = **slot;
            if idx < functions.len() {
                crate::jit::name_host_call_slot(functions[idx], func_name);
            }
        }
        crate::jit::name_host_call_slot(vm_functions[VM_GET_ENV], "JavaVM.GetEnv");

        let env = object2(env_fn_tbl);
        let vm = object2(vm_fn_tbl);
        (env, vm)
    })
}

fn reg(f: HostCall) -> u64 {
    register_host_call_auto(f)
}

/// Register a JNI float-return bridge in the dedicated s0-return thunk region.
fn reg_jni_f32(f: HostJniF32) -> u64 {
    crate::jit::register_jni_f32_call(f)
}

/// Allocate (host==guest) memory for a u64 table; return its address.
fn u64array(items: &[u64]) -> u64 {
    let n = items.len();
    let p = unsafe { alloc_zeroed(Layout::array::<u64>(n).unwrap()) } as *mut u64;
    for (i, v) in items.iter().enumerate() {
        unsafe { *p.add(i) = *v };
    }
    p as u64
}

/// Allocate a 5-word object whose word0 = `functions` pointer (JNIEnv/JavaVM).
fn object2(functions: u64) -> u64 {
    let p = unsafe { alloc_zeroed(Layout::new::<[u64; 5]>()) } as *mut u64;
    unsafe { *p = functions };
    p as u64
}

/// The guest address `vm->GetEnv(&env, v)` should write into `*penv`; the guest
/// JNIEnv (idempotent). Convenience for boot glue.
pub fn env_addr() -> u64 {
    let (e, _v) = build_jni();
    e
}

/// Allocate a fake-but-valid (non-null, dereferenceable, zeroed) `jobject` the
/// guest can pass around / store without faulting — the JIT's analogue of the
/// JVM handing an Activity `jobject` to a native method. Word0 stays 0 (no
/// v-table), which is honest for the current fake-object model. For boot glue
/// that must pass *some* activity-like handle as x1 to a Java_* entry.
pub fn new_fake_object() -> u64 {
    let p = unsafe { alloc_zeroed(Layout::new::<[u64; 8]>()) } as u64;
    p
}

/// Allocate a guest-addressable, null-terminated jstring handle for the given
/// UTF-8 bytes; returns the handle (str_handle semantics, same bytes -> same
/// handle). For boot glue passing a params jstring to a Java_* entry.
pub fn new_string_utf_handle(bytes: &[u8]) -> u64 {
    str_handle(bytes)
}

/// Dispatch a registered native method back into the guest through `jit_run`.
///
/// `lookup_native_method` resolves (class,name) to a guest `fnPtr` (a Java_*
/// implementation the guest bound via `RegisterNatives`). This runs it as a
/// JIT guest entry with `base`/`image` covering the loaded ELF, passing the
/// caller-supplied trace/region. Returns the guest x0 after the callee's
/// `ret`. This is the glue that turns a *recorded* Java_* binding — which
/// `jni_register_natives` now captures — into a *callable* guest function,
/// which is what a host Android-runtime dispatch of a Roblox native method
/// needs alongside a real fake-object backing.
///
/// Returns `Err` if the method has no binding or `jit_run` stops (unmapped /
/// unsupported).
pub fn dispatch_native_method(
    method: &NativeMethod,
    base: u64,
    image: &[u8],
    args: [u64; 8],
    tpidr: u64,
) -> Result<u64, String> {
    if method.fn_ptr == 0 {
        return Err(format!(
            "native method {}:{} has NULL fnPtr",
            String::from_utf8_lossy(&method.class),
            String::from_utf8_lossy(&method.name)
        ));
    }
    let mut st = crate::jit::CpuState::new();
    st.tpidr = tpidr;
    st.x[..8].copy_from_slice(&args);
    crate::jit::jit_run(image, base, method.fn_ptr, &mut st as *mut crate::jit::CpuState)?;
    Ok(st.x[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jit::host_call_at;

    /// Every slot the guest's JNI_OnLoad path reaches must be a non-null host
    /// thunk at the OFFICIAL ABI offset (a mis-slot dispatches to NULL -> crash).
    #[test]
    fn jni_table_has_official_abi_slots_nonnull() {
        let (env, _vm) = build_jni();
        unsafe {
            let functions = *(env as *const u64);
            let get = |i: usize| -> u64 { *(functions as *const u64).add(i) };
            for (i, name) in [
                (GET_VERSION, "GetVersion"),
                (FIND_CLASS, "FindClass"),
                (GET_METHOD_ID, "GetMethodID"),
                (GET_STATIC_METHOD_ID, "GetStaticMethodID"),
                (NEW_STRING_UTF, "NewStringUTF"),
                (GET_STRING_UTF_CHARS, "GetStringUTFChars"),
                (REGISTER_NATIVES, "RegisterNatives"),
                (GET_JAVA_VM, "GetJavaVM"),
            ] {
                let sl = get(i);
                assert!(sl != 0, "slot {i} ({name}) non-null");
                host_call_at(sl).expect(name);
            }
            // The VOIDPIN default NULL-slot must be absent from the boot-relevant names.
            assert!(get(NEW_STRING_UTF) != get(FIND_CLASS), "distinct stubs");
        }
    }

    #[test]
    fn jni_new_string_utf_is_readable() {
        // Invoke the NewStringUTF stub directly: it must return a readable,
        // null-terminated handle containing the UTF bytes, not a low sentinel.
        let payload = b"com/roblox/engine/jni/user/NativeUserJavaInterface";
        let src = unsafe { alloc_zeroed(Layout::array::<u8>(payload.len() + 1).unwrap()) };
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), src, payload.len()) };
        let handle = jni_new_string_utf(0, src as u64, 0, 0, 0, 0, 0, 0);
        assert!(handle != 0 && handle > 0x1000, "readable handle, not sentinel");
        assert_eq!(read_cstr(handle).as_deref(), Some(&payload[..]));
        // idempotent across calls
        assert_eq!(handle, jni_new_string_utf(0, src as u64, 0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn jni_register_natives_records_guest_bindings() {
        // Build a real JNINativeMethod array (3 u64 words each: name*,
        // signature*, fnPtr) in guest memory, exactly as JNI_OnLoad does, and
        // verify the registry parses it and lookup_native_method returns the
        // guest fnPtr (a Java_* address the host would jit_run).
        use std::alloc::{alloc, Layout};
        let mut w = |bytes: &[u8]| -> u64 {
            let p = unsafe { alloc(Layout::array::<u8>(bytes.len() + 1).unwrap()) } as *mut u8;
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
            unsafe { *p.add(bytes.len()) = 0 };
            p as u64
        };
        let cls = w(b"com/roblox/engine/jni/iap/IAPPurchaseManager");
        let name1 = w(b"nativeInit");
        let sig1 = w(b"(J)V");
        let fn1 = 0x1000_5f00u64; // a guest .text address
        let name2 = w(b"nativePurchase");
        let sig2 = w(b"(Ljava/lang/String;)Z");
        let fn2 = 0x1000_7200u64;
        let methods = {
            let p = unsafe { alloc(Layout::array::<u64>(6).unwrap()) } as *mut u64;
            let src = [name1, sig1, fn1, name2, sig2, fn2];
            for (i, v) in src.iter().enumerate() {
                unsafe { *p.add(i) = *v };
            }
            p as u64
        };

        clear_native_methods();
        assert_eq!(jni_register_natives(0, cls, methods, 2, 0, 0, 0, 0), 0); // JNI_OK

        let m = lookup_native_method(
            b"com/roblox/engine/jni/iap/IAPPurchaseManager",
            b"nativeInit",
        )
        .expect("nativeInit bound");
        assert_eq!(m.fn_ptr, fn1);
        assert_eq!(m.signature, b"(J)V");
        assert_eq!(
            lookup_native_method(
                b"com/roblox/engine/jni/iap/IAPPurchaseManager",
                b"nativePurchase",
            )
            .unwrap()
            .fn_ptr,
            fn2
        );
        // Unknown name -> None.
        assert!(lookup_native_method(b"x", b"no_such").is_none());
        // Last registration wins (re-register with a different fnPtr).
        let name3 = w(b"nativeInit");
        let p3 = unsafe { alloc(Layout::array::<u64>(3).unwrap()) } as *mut u64;
        unsafe {
            *p3 = name3;
            *p3.add(1) = sig1;
            *p3.add(2) = 0x2000_1111u64;
        }
        jni_register_natives(0, cls, p3 as u64, 1, 0, 0, 0, 0);
        assert_eq!(
            lookup_native_method(
                b"com/roblox/engine/jni/iap/IAPPurchaseManager",
                b"nativeInit",
            )
            .unwrap()
            .fn_ptr,
            0x2000_1111u64,
            "re-registration replaces"
        );
    }

    #[test]
    fn jni_entry_point_builds() {
        let (env, vm) = build_jni();
        assert!(env > 0 && vm > 0);
    }

    /// Object-array accessors: NewObjectArray seeds the backing with `init`,
    /// Set/GetObjectArrayElement round-trip an opaque jobject handle, and an
    /// out-of-range index is bounds-checked (get -> NULL, set -> no-op) instead of
    /// faulting. This was `voidp`/`voidp`/`ok` (returning garbage) before wired.
    #[test]
    fn jni_object_array_roundtrip_and_bounds() {
        use super::jni_new_object_array;
        // NewObjectArray(len=3, class=NULL, init=someHandle) -> backing buffer.
        let init: u64 = 0x1234_5678_9abc_def0;
        let arr = jni_new_object_array(0, 3, 0, init, 0, 0, 0, 0);
        assert_ne!(arr, 0, "NewObjectArray returned a backing buffer");

        // init seeded every element.
        let v0 = super::jni_get_object_array_element(0, arr, 0, 0, 0, 0, 0, 0);
        let v1 = super::jni_get_object_array_element(0, arr, 1, 0, 0, 0, 0, 0);
        let v2 = super::jni_get_object_array_element(0, arr, 2, 0, 0, 0, 0, 0);
        assert_eq!(v0, init, "element 0 seeded with initialElement");
        assert_eq!(v1, init, "element 1 seeded");
        assert_eq!(v2, init, "element 2 seeded");

        // SetObjectArrayElement(index=1, newHandle) round-trips.
        let newh: u64 = 0xdead_beef_cafe_1234;
        super::jni_set_object_array_element(0, arr, 1, newh, 0, 0, 0, 0);
        assert_eq!(
            super::jni_get_object_array_element(0, arr, 1, 0, 0, 0, 0, 0),
            newh,
            "Set/GetObjectArrayElement round-trip"
        );
        // Other elements untouched.
        assert_eq!(super::jni_get_object_array_element(0, arr, 0, 0, 0, 0, 0, 0), init);

        // Bounds: index 3 (== len) is out of range.
        assert_eq!(
            super::jni_get_object_array_element(0, arr, 3, 0, 0, 0, 0, 0),
            0,
            "out-of-range get returns NULL"
        );
        // Set at a bogus index must not corrupt valid memory.
        super::jni_set_object_array_element(0, arr, 3, 42, 0, 0, 0, 0);
        assert_eq!(super::jni_get_object_array_element(0, arr, 2, 0, 0, 0, 0, 0), init);
    }

    /// End-to-end: register a native method binding whose guest fnPtr is a
    /// real aarch64 function (`mov x0,#0x2a; ret`), then dispatch it through
    /// jit_run and verify the guest x0 comes back. Proves the recorded
    /// fnPtr (guested address) is directly runnable as a JIT entry.
    #[test]
    fn jni_dispatch_registered_native_method_through_jit() {
        // Function: mov x0,#0x2a (42); ret
        let code: [u8; 8] = [
            0x40, 0x05, 0x80, 0xd2, // mov x0, #0x2a (d2800540)
            0xc0, 0x03, 0x5f, 0xd6, // ret
        ];
        let base = 0x1000u64;
        clear_native_methods();
        // Register a binding pointing into the `code` buffer as guest/home
        // (guest==host here). Build the JNINativeMethod array.
        use std::alloc::{alloc, Layout};
        let mut w = |bytes: &[u8]| -> u64 {
            let p = unsafe { alloc(Layout::array::<u8>(bytes.len() + 1).unwrap()) } as *mut u8;
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
            unsafe { *p.add(bytes.len()) = 0 };
            p as u64
        };
        let cls = w(b"com/example/Native");
        let name = w(b"answer");
        let sig = w(b"()I");
        let arr = unsafe { alloc(Layout::array::<u64>(3).unwrap()) } as *mut u64;
        unsafe {
            *arr = name;
            *arr.add(1) = sig;
            *arr.add(2) = base; // fnPtr = the guest code we JIT
        }
        jni_register_natives(0, cls, arr as u64, 1, 0, 0, 0, 0);

        let m = lookup_native_method(b"com/example/Native", b"answer").expect("bound");
        let ret = dispatch_native_method(&m, base, &code, [0; 8], 0).expect("dispatch");
        assert_eq!(ret, 42, "guest Java_* method executed through jit_run");
    }

    #[test]
    fn jni_returns_same_instance() {
        assert_eq!(env_addr(), env_addr(), "build_jni idempotent");
    }

    /// End-to-end: JIT-execute guest aarch64 that does the `JNI_OnLoad` preamble —
    /// `JavaVM*` in x0, call vm->GetEnv(&env, 0x10006), then env->GetVersion(),
    /// then env->NewStringUTF — and verify the results land in registers. Uses the
    /// OFFICIAL slot offsets (vm GetEnv=6, env GetVersion=4, NewStringUTF=167).
    #[test]
    fn jit_jni_onload_getenv_getversion() {
        use crate::jit::{jit_run, CpuState};
        let (_env, vm) = build_jni();
        let scratch = unsafe { std::alloc::alloc_zeroed(std::alloc::Layout::new::<u64>()) };
        let scratch_addr = scratch as u64;

        let mut st = CpuState::new();
        st.x[0] = vm; // JavaVM*
        st.x[1] = scratch_addr; // penv
        st.x[2] = 0x10006; // version

        // aarch64 (official ABI offsets):
        //   ldr x10,[x0]         ; x10 = vm function table
        //   ldr x9,[x10,#48]     ; x9  = vm_table[6] = GetEnv (JNIInvokeInterface)
        //   blr x9               ; GetEnv(vm,&penv,version)
        //   ldr x13,[x1]         ; x13 = env (JNIEnv*)
        //   ldr x13,[x13]        ; x13 = env->functions (JNIEnv fn table)
        //   ldr x14,[x13,#32]    ; x14 = env_table[4] = GetVersion
        //   blr x14              ; GetVersion -> x0 = 0x10006
        let mut code: Vec<u8> = Vec::new();
        let mut w = |i: u32| code.extend_from_slice(&i.to_le_bytes());
        w(0xf940000a); // ldr x10,[x0]
        w(0xf9401949); // ldr x9,[x10,#48]  (vm_table[6] = GetEnv, byte 48)
        w(0xd63f0120); // blr x9
        w(0xf940002d); // ldr x13,[x1]      -> x13 = env
        w(0xf94001ad); // ldr x13,[x13]     -> x13 = env->functions
        w(0xf94011ae); // ldr x14,[x13,#32] (env_table[4] = GetVersion)
        w(0xd63f01c0); // blr x14           -> x0 = 0x10006
        w(0xd4200000); // brk #0

        let res = jit_run(&code, 0x1000, 0x1000, &mut st as *mut CpuState);
        assert!(res.is_ok(), "jit_run over JNI_OnLoad guest step: {res:?}");
        assert_eq!(st.x[0], JNI_VERSION_1_6 as u64, "GetVersion through JIT = 0x10006");
    }

    /// The primitive-array offsets must match the authoritative Android NDK jni.h
    /// (not the JDK table, which places Set*ArrayRegion differently). libroblox.so
    /// indexes the JNINativeInterface by NDK word offsets, so a wrong constant ducks
    /// RegisterNatives/GetJavaVM onto the wrong slot -> the guest calls garbage.
    #[test]
    fn jni_array_offsets_match_android_ndk() {
        assert_eq!(NEW_BYTE_ARRAY, 176, "NewByteArray (NDK)");
        assert_eq!(NEW_INT_ARRAY, 179, "NewIntArray (NDK)");
        assert_eq!(GET_BYTE_ARRAY_ELEMENTS, 184, "GetByteArrayElements (NDK)");
        assert_eq!(GET_INT_ARRAY_ELEMENTS, 187, "GetIntArrayElements (NDK)");
        assert_eq!(GET_BYTE_ARRAY_REGION, 200, "GetByteArrayRegion (NDK)");
        assert_eq!(GET_INT_ARRAY_REGION, 203, "GetIntArrayRegion (NDK)");
        assert_eq!(SET_BYTE_ARRAY_REGION, 208, "SetByteArrayRegion (NDK)");
        assert_eq!(SET_INT_ARRAY_REGION, 211, "SetIntArrayRegion (NDK)");
        assert_eq!(REGISTER_NATIVES, 215, "RegisterNatives (NDK)");
        assert_eq!(GET_JAVA_VM, 219, "GetJavaVM (NDK)");
    }

    /// NewByteArray -> GetByteArrayElements -> SetByteArrayRegion -> GetByteArrayRegion
    /// must round-trip bytes through the array backing (jbyteArray = readable buffer).
    #[test]
    fn byte_array_region_roundtrip() {
        let arr = jni_new_array_raw(8, 1);
        assert!(arr != 0, "NewByteArray allocs a handle");
        // Set the backing: write bytes [10,20,30,40] at offset 0.
        let src = [10u8, 20, 30, 40];
        jni_set_byte_array_region(0, arr, 0, 4, src.as_ptr() as u64, 0, 0, 0);
        let mut dst = [0u8; 4];
        jni_get_byte_array_region(0, arr, 0, 4, dst.as_mut_ptr() as u64, 0, 0, 0);
        assert_eq!(dst, src, "Set/GetByteArrayRegion round-trips bytes");
        // GetArrayLength reports the byte length for a byte array.
        assert_eq!(jni_get_array_length(0, arr, 0, 0, 0, 0, 0, 0), 8);
        // Bounds check: an out-of-range region must not copy (no crash).
        let mut out = [0u8; 4];
        jni_get_byte_array_region(0, arr, 6, 4, out.as_mut_ptr() as u64, 0, 0, 0);
        assert_eq!(out, [0u8; 4], "out-of-range Get did not write");
    }

    /// GetByteArrayElements returns the same backing pointer the constructor made
    /// (isCopy=0), so the guest can read/write it directly.
    #[test]
    fn byte_array_elements_returns_backing() {
        let arr = jni_new_array_raw(4, 1);
        let mut iscopy = 5i8;
        let p = jni_get_byte_array_elements(0, arr, &mut iscopy as *mut i8 as u64, 0, 0, 0, 0, 0);
        assert_eq!(p, arr, "GetByteArrayElements returns the array backing");
        assert_eq!(iscopy, 0, "isCopy set to 0 (no copy)");
        // Writing through the pointer is visible via the registry-backed length.
        unsafe { *(arr as *mut u8) = 0xAB };
        assert_eq!(jni_get_array_length(0, arr, 0, 0, 0, 0, 0, 0), 4);
    }

    /// The added fake-object / monitor / exception / ref / static-field /
    /// direct-buffer slot offsets must match the authoritative Android NDK.
    #[test]
    fn jni_fake_object_surface_offsets_match_android_ndk() {
        assert_eq!(IS_ASSIGNABLE_FROM, 11, "IsAssignableFrom (NDK)");
        assert_eq!(EXCEPTION_OCCURRED, 15, "ExceptionOccurred");
        assert_eq!(EXCEPTION_CLEAR, 17, "ExceptionClear");
        assert_eq!(NEW_LOCAL_REF, 25, "NewLocalRef");
        assert_eq!(IS_SAME_OBJECT, 24, "IsSameObject");
        assert_eq!(GET_OBJECT_CLASS, 31, "GetObjectClass");
        assert_eq!(IS_INSTANCE_OF, 32, "IsInstanceOf");
        assert_eq!(CALL_OBJECT_METHOD, 34, "CallObjectMethod (NDK)");
        assert_eq!(CALL_INT_METHOD, 49, "CallIntMethod (NDK)");
        assert_eq!(CALL_LONG_METHOD, 52, "CallLongMethod (NDK)");
        assert_eq!(CALL_FLOAT_METHOD, 55, "CallFloatMethod (NDK)");
        assert_eq!(CALL_VOID_METHOD, 61, "CallVoidMethod (NDK)");
        assert_eq!(CALL_STATIC_OBJECT_METHOD, 114, "CallStaticObjectMethod");
        assert_eq!(CALL_STATIC_INT_METHOD, 129, "CallStaticIntMethod");
        assert_eq!(CALL_STATIC_VOID_METHOD, 141, "CallStaticVoidMethod");
        assert_eq!(GET_STATIC_FIELD_ID, 144, "GetStaticFieldID");
        assert_eq!(GET_STATIC_OBJECT_FIELD, 145, "GetStaticObjectField");
        assert_eq!(RELEASE_STRING_UTF_CHARS, 170, "ReleaseStringUTFChars");
        assert_eq!(MONITOR_ENTER, 217, "MonitorEnter");
        assert_eq!(MONITOR_EXIT, 218, "MonitorExit");
        assert_eq!(GET_STRING_UTF_REGION, 221, "GetStringUTFRegion");
        assert_eq!(EXCEPTION_CHECK, 228, "ExceptionCheck");
        assert_eq!(NEW_DIRECT_BYTE_BUFFER, 229, "NewDirectByteBuffer");
        assert_eq!(GET_DIRECT_BUFFER_ADDRESS, 230, "GetDirectBufferAddress");
        assert_eq!(GET_DIRECT_BUFFER_CAPACITY, 231, "GetDirectBufferCapacity");
    }

    /// The boot-relevant fake-object surface must be non-null host thunks at the
    /// official offsets (a mis-slot or NULL dispatches to 0 -> guest abort).
    #[test]
    fn jni_fake_object_slots_are_nonnull_at_official_offsets() {
        let (env, _vm) = build_jni();
        unsafe {
            let functions = *(env as *const u64);
            let get = |i: usize| -> u64 { *(functions as *const u64).add(i) };
            for (i, name) in [
                (NEW_LOCAL_REF, "NewLocalRef"),
                (IS_SAME_OBJECT, "IsSameObject"),
                (GET_OBJECT_CLASS, "GetObjectClass"),
                (IS_INSTANCE_OF, "IsInstanceOf"),
                (MONITOR_ENTER, "MonitorEnter"),
                (MONITOR_EXIT, "MonitorExit"),
                (EXCEPTION_OCCURRED, "ExceptionOccurred"),
                (EXCEPTION_CLEAR, "ExceptionClear"),
                (EXCEPTION_CHECK, "ExceptionCheck"),
                (GET_STRING_UTF_REGION, "GetStringUTFRegion"),
                (NEW_DIRECT_BYTE_BUFFER, "NewDirectByteBuffer"),
                (GET_DIRECT_BUFFER_ADDRESS, "GetDirectBufferAddress"),
                (GET_DIRECT_BUFFER_CAPACITY, "GetDirectBufferCapacity"),
                // Call-method family (typed-zero / void) must not be the voidp=NULL.
                (CALL_OBJECT_METHOD, "CallObjectMethod"),
                (CALL_INT_METHOD, "CallIntMethod"),
                (CALL_VOID_METHOD, "CallVoidMethod"),
                (CALL_STATIC_VOID_METHOD, "CallStaticVoidMethod"),
            ] {
                let sl = get(i);
                assert!(sl != 0, "slot {i} ({name}) non-null");
                host_call_at(sl).expect(name);
            }
        }
    }

    /// Fake-object backing semantics: NewLocalRef/NewGlobalRef are identity,
    /// IsSameObject is identity-compare, GetObjectClass returns a stable non-zero
    /// class handle, IsInstanceOf/IsAssignableFrom are permissive true, and the
    /// exception/monitor slots report no pending state.
    #[test]
    fn jni_fake_object_backing_semantics() {
        use super::{
            jni_get_object_class, jni_is_instance_of, jni_is_same_object,
            jni_new_local_ref, jni_self_true,
        };
        let obj: u64 = 0x1000_abcd;
        // NewLocalRef passthrough.
        assert_eq!(jni_new_local_ref(0, obj, 0, 0, 0, 0, 0, 0), obj);
        // IsSameObject identity.
        assert_eq!(jni_is_same_object(0, obj, obj, 0, 0, 0, 0, 0), 1);
        assert_eq!(jni_is_same_object(0, obj, 0xdead, 0, 0, 0, 0, 0), 0);
        // GetObjectClass: non-zero, stable, distinct from the object; 0 for NULL.
        let c = jni_get_object_class(0, obj, 0, 0, 0, 0, 0, 0);
        assert_ne!(c, 0, "GetObjectClass returns a handle");
        assert_ne!(c, obj, "class handle does not alias the object");
        assert_eq!(jni_get_object_class(0, c, 0, 0, 0, 0, 0, 0), c, "stable");
        assert_eq!(jni_get_object_class(0, 0, 0, 0, 0, 0, 0, 0), 0, "NULL obj -> NULL class");
        // Permissive instanceof / assignable-from.
        assert_eq!(jni_is_instance_of(0, obj, c, 0, 0, 0, 0, 0), 1);
        assert_eq!(jni_self_true(0, 0, 0, 0, 0, 0, 0, 0), 1);
    }

    /// Direct NIO ByteBuffer round-trip: NewDirectByteBuffer records the backing,
    /// GetDirectBufferAddress/Capacity recover it, and an unknown handle yields 0.
    #[test]
    fn jni_direct_byte_buffer_round_trip() {
        use super::{
            jni_get_direct_buffer_address, jni_get_direct_buffer_capacity,
            jni_new_direct_byte_buffer,
        };
        let backing: u64 = 0x6000_3000;
        let cap = 1024u64;
        let h = jni_new_direct_byte_buffer(0, backing, cap, 0, 0, 0, 0, 0);
        assert_ne!(h, 0, "NewDirectByteBuffer returns a handle");
        assert_ne!(h, backing, "handle is distinct from the backing address");
        assert_eq!(jni_get_direct_buffer_address(0, h, 0, 0, 0, 0, 0, 0), backing);
        assert_eq!(jni_get_direct_buffer_capacity(0, h, 0, 0, 0, 0, 0, 0), cap);
        // Unknown handle -> 0 (not-a-direct-buffer).
        assert_eq!(jni_get_direct_buffer_address(0, 0x999, 0, 0, 0, 0, 0, 0), 0);
        // Two NewDirectByteBuffer calls yield distinct handles.
        let h2 = jni_new_direct_byte_buffer(0, backing, cap, 0, 0, 0, 0, 0);
        assert_ne!(h, h2, "fresh unique handle per allocation");
        assert_eq!(jni_get_direct_buffer_address(0, h2, 0, 0, 0, 0, 0, 0), backing);
    }

    /// GetStringUTFRegion copies `len` UTF-8 bytes at byte `start` into `buf`,
    /// bounds-safe (out-of-range start leaves buf untouched, no fault).
    #[test]
    fn jni_get_string_utf_region_copies() {
        use super::jni_get_string_utf_region;
        let s = b"ro.blox engine";
        let src = unsafe { alloc_zeroed(Layout::array::<u8>(s.len() + 1).unwrap()) };
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), src, s.len()) };
        let mut buf = [0u8; 5];
        // Copy "blox" (bytes 3..7).
        jni_get_string_utf_region(0, src as u64, 3, 4, buf.as_mut_ptr() as u64, 0, 0, 0);
        assert_eq!(&buf[..4], b"blox", "region copy");
        assert_eq!(buf[4], 0, "only len bytes copied");
        // Out-of-range start -> no copy, no crash.
        let mut untouched = [0u8; 3];
        jni_get_string_utf_region(0, src as u64, 99, 3, untouched.as_mut_ptr() as u64, 0, 0, 0);
        assert_eq!(untouched, [0u8; 3]);
    }

    /// The AutoValue AppBridge params getters — the exact Accessors StartApp's
    /// json serialization reads through CallObjectMethod/CallBooleanMethod/
    /// CallIntMethod — must return real, readable, non-NULL values through the
    /// OFFICIAL JNIEnv table (the way the guest dispatches them), so the json
    /// writer reads a valid empty/default string length instead of an
    /// uninitialized guest-stack std::string (the SH45/SH46 string-length-
    /// overflow root cause). InitParams/StartAppParams fields resolve to
    /// readable jstring handles; unknown method names still fall back to 0.
    #[test]
    fn jni_auto_value_params_getters_resolve_via_fn_table() {
        let (env, _vm) = build_jni();
        unsafe {
            let functions = *(env as *const u64);
            let get = |i: usize| -> u64 { *(functions as *const u64).add(i) };
            // The guest resolves a getter method ID, then Call*Method(..id..).
            // GetMethodID returns a readable handle of the method NAME.
            let get_name_id = |name: &[u8]| -> u64 {
                let name_buf = unsafe { alloc_zeroed(Layout::array::<u8>(name.len() + 1).unwrap()) };
                unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), name_buf, name.len()) };
                let (f, _) = host_call_at(get(GET_METHOD_ID)).expect("GetMethodID host thunk");
                f(env, 0, name_buf as u64, 0, 0, 0, 0, 0)
            };
            // String-valued StartAppParams/InitParams getters -> real jstring
            // handles (readable, non-zero), so the json writer sees byte length.
            for name in [
                &b"getBaseURL"[..],
                &b"getBuildVariant"[..],
                &b"getUserAgent"[..],
                &b"getAppStarterPlace"[..],
                &b"getAppStarterScript"[..],
                &b"getSelectedTheme"[..],
                &b"getUsername"[..],
                &b"getAppUserId"[..],
                &b"getDeviceParams"[..],
                &b"getPlatformParams"[..],
                &b"getVrContext"[..],
                &b"getSurface"[..],
                &b"getOsVersion"[..],
                &b"getDeviceName"[..],
                &b"getDeviceSku"[..],
                &b"getManufacturer"[..],
                &b"getCountry"[..],
                &b"getNetworkType"[..],
                &b"getAppVersion"[..],
            ] {
                let mid = get_name_id(name);
                assert_ne!(mid, 0, "GetMethodID({}) non-null", String::from_utf8_lossy(name));
                let (g_om, _) = host_call_at(get(CALL_OBJECT_METHOD)).expect("CallObjectMethod thunk");
                let h = g_om(env, 0x4321, mid, 0, 0, 0, 0, 0);
                assert_ne!(h, 0, "CallObjectMethod({}) returns a readable jstring", String::from_utf8_lossy(name));
                // The returned handle is a readable zero-length (or for
                // selectedTheme "Dark", getOsVersion "33") jstring —
                // GetStringUTFLength reads it.
                let (g_sl, _) = host_call_at(get(GET_STRING_UTF_LEN)).expect("GetStringUTFLength thunk");
                let len = g_sl(env, h, 0, 0, 0, 0, 0, 0);
                match name {
                    b"getSelectedTheme" => assert_eq!(len, 4, "selectedTheme defaults to \"Dark\""),
                    b"getOsVersion" => assert_eq!(len, 2, "osVersion defaults to \"33\""),
                    b"getDeviceName" => assert_eq!(len, 7, "getDeviceName \"Cordial\""),
                    b"getDeviceSku" => assert_eq!(len, 7, "getDeviceSku \"cordial\""),
                    b"getManufacturer" => assert_eq!(len, 7, "getManufacturer \"Cordial\""),
                    b"getCountry" => assert_eq!(len, 2, "getCountry \"US\""),
                    b"getNetworkType" => assert_eq!(len, 4, "getNetworkType \"WIFI\""),
                    _ => assert_eq!(len, 0, "{} defaults to empty", String::from_utf8_lossy(name)),
                }
            }
            // Boolean getters -> per recon v2 shape (isUnder13 false, keyboard
            // true); int getter getMembershipType -> 0; long getter
            // getAppUserId -> 0.
            let (g_bm, _) = host_call_at(get(CALL_BOOLEAN_METHOD)).expect("CallBooleanMethod thunk");
            assert_eq!(g_bm(env, 0x4321, get_name_id(b"isUnder13"), 0, 0, 0, 0, 0), 0, "isUnder13 false");
            assert_eq!(g_bm(env, 0x4321, get_name_id(b"isTouchDevice"), 0, 0, 0, 0, 0), 0, "isTouchDevice false");
            assert_eq!(g_bm(env, 0x4321, get_name_id(b"isKeyboardDevice"), 0, 0, 0, 0, 0), 1, "isKeyboardDevice true");
            let mid2 = get_name_id(b"getMembershipType");
            let (g_im, _) = host_call_at(get(CALL_INT_METHOD)).expect("CallIntMethod thunk");
            assert_eq!(g_im(env, 0x4321, mid2, 0, 0, 0, 0, 0), 0, "getMembershipType default 0");
            let (g_lm, _) = host_call_at(get(CALL_LONG_METHOD)).expect("CallLongMethod thunk");
            assert_eq!(g_lm(env, 0x4321, get_name_id(b"getAppUserId"), 0, 0, 0, 0, 0), 0, "getAppUserId default 0");
            // getAllocatableBytes (LocalStorageManager) reports REAL host free
            // space — recon-v2: 0 means the engine believes there's no disk and
            // RbxStorage never builds its content cache (objective 2b). Must be
            // a positive, plausible live byte count, not the old 0 collapse.
            let (g_lm2, _) = host_call_at(get(CALL_LONG_METHOD)).expect("CallLongMethod thunk");
            let free = g_lm2(env, 0x4321, get_name_id(b"getAllocatableBytes"), 0, 0, 0, 0, 0);
            assert!(free > 0, "getAllocatableBytes reports real (non-zero) free bytes, got {free}");
            let (_bsize, bavail) = {
                let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
                let rc = unsafe { libc::statvfs(b"/\0".as_ptr() as *const libc::c_char, &mut st) };
                assert_eq!(rc, 0, "statvfs / works");
                (st.f_frsize as u64, st.f_bavail as u64)
            };
            assert!(
                free <= (bavail.saturating_mul(8) * 1024 * 1024 * 1024),
                "free bytes within a sane order of magnitude of the host disk"
            );
            // Unrecognized method name still returns 0 (legacy fallback) — so
            // unrelated Call*Method sites (real Java re-entry) are unchanged.
            let midx = get_name_id(b"someOtherMethod");
            let (g_om2, _) = host_call_at(get(CALL_OBJECT_METHOD)).expect("CallObjectMethod thunk");
            assert_eq!(g_om2(env, 0x4321, midx, 0, 0, 0, 0, 0), 0, "unrecognized getter falls back to NULL/0");
        }
    }

    /// The NDK JNIEnv table dispatches CallFloatMethod through slot 55, but
    /// that slot is a HostJniF32 bridge (in the s0-return thunk region), NOT an
    /// integer hostcall — so it must be non-null and distinct, and the guest
    /// reading s0 after the blr gets the real jfloat. This JIT-runs real guest
    /// aarch64 that does `CallFloatMethod(env, obj, getDpiScale)` through the
    /// official table and returns the float bits via `fmov w0, s0`.
    #[test]
    fn jni_call_float_method_returns_jfloat_in_s0_via_jit() {
        use crate::jit::{jit_run, CpuState};
        let (env, _vm) = build_jni();
        unsafe {
            let functions = *(env as *const u64);
            let f_slot = *(functions as *const u64).add(CALL_FLOAT_METHOD);
            assert_ne!(f_slot, 0, "CallFloatMethod slot non-null");
            // It must be a float-return thunk, not an integer hostcall slot.
            assert_ne!(f_slot, *(functions as *const u64).add(CALL_OBJECT_METHOD));
        }
        let mid = new_string_utf_handle(b"getDpiScale");
        let obj: u64 = 0x4321;

        // base=0x1000; x0=env, x1=obj, x2=mid.
        //   ldr x9,[x0]         ; functions = [env]
        //   ldr x10,[x9,#440]   ; functions[55] = CallFloatMethod (55*8)
        //   blr x10             ; -> s0 = 1.0f32 bits
        //   fmov w0,s0          ; w0 = s0 bits (float return -> GPR for assert)
        //   brk #0              ; halt -> pc=0, jit_run returns Ok(x0)
        let code: [u32; 5] = [
            0xf9400009, // ldr x9,[x0]
            0xf940dd2a, // ldr x10,[x9,#440]  (55<<3)
            0xd63f0140, // blr x10
            0x1e260000, // fmov w0,s0
            0xd4200000, // brk #0 (halt)
        ];
        let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut st = CpuState::new();
        st.x[0] = env;
        st.x[1] = obj;
        st.x[2] = mid;
        let res = jit_run(&bytes, 0x1000, 0x1000, &mut st as *mut CpuState);
        assert!(res.is_ok(), "jit_run over CallFloatMethod: {res:?}");
        // s0 (v0 low 32) must hold 1.0f32 bits after the dispatch.
        assert_eq!(st.v[0] & 0xffff_ffff, 1.0f32.to_bits() as u64, "s0 = getDpiScale 1.0");
        assert_eq!(st.x[0] & 0xffff_ffff, 1.0f32.to_bits() as u64, "fmov w0,s0 saw 1.0");
    }
}