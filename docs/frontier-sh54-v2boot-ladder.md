# SH54 — Ordered V2-boot ladder drive + AutoValue getter shim

Session: Sep 12, 2026, hermes-worker, cycle SH54. Workspace **498/0** (was 497/0, +1
regression). Code: `crates/arm64jit/src/jni.rs` (AutoValue getter shim),
`crates/arm64jit/examples/elfjit.rs` (`--v2boot` ordered ladder). Artifacts:
`runs/sh54-v2boot-ladder.txt` (exit 124 stable, vector 0 throughout),
`runs/sh54-v2boot-fw.txt` (JIT_FRAMEWORK_DUMP per-window vector samples),
`runs/sh54-v2boot-progress.txt`, `runs/sh54-v2boot-threads.txt`,
`runs/capture_v2boot.sh`.

## What was implemented

The immediate-priority recon (docs/recon-framework-boot-order.md) demands the
real V2 boot sequence be driven **in order** — not bare/out-of-order
StartAppWithParams with a JSON string where a JNI AutoValue jobject is expected
(the documented json-abort root cause). SH53 had closed the *static* scan of the
install site but explicitly left the empirical drive open as "a cross-module /
runtime test". This cycle builds the two pieces that make that drive honest:

1. **AutoValue getter shim (jni.rs).** The engine's V2 ladder reads its
   InitParams/StartAppParams as Java AutoValue objects via `GetMethodID` then
   `CallObjectMethod`/`CallBooleanMethod`/`CallIntMethod` on each field. The JNI
   table's `Call*Method` slots all returned **0**, so every AutoValue getter
   returned NULL/0 and StartApp's json serialization read uninitialized
   guest-stack `std::string`s — the exact SH45/SH46 `RBX::json::Writer string
   length overflow` abort. Because `jni_get_method_id` already returns a
   **readable handle of the method NAME**, the new `CallObjectMethod`/
   `CallBooleanMethod`/`CallIntMethod` stubs dispatch on the getter name and
   return a real, readable, empty (or `"Dark"` for `getSelectedTheme`) jstring /
   false / 0 — so the json writer reads a valid byte-length instead of a stack
   pointer. Unrecognized names still fall back to 0 (real Java re-entry
   unchanged). Wired into the OFFICIAL NDK table slots 34/37/49.

2. **`--v2boot` ordered ladder (elfjit.rs).** A detached thread (spawned BEFORE
   the `start_app` jit_run, which parks the main thread forever) sleeps a warmup
   then drives the real JNI natives in the recon's load-bearing order, each as a
   fresh guest entry reusing the boot SP, dumping `[0x106829ea8]` **after every
   rung**:
   nativeGameGlobalInit (0x102206404) → nativeUpdateAdapterInit (0x10221c3ec) →
   setTaskSchedulerBackgroundMode(false,"ASMA.start") (0x102bb2380) →
   V2InitWithParams (0x102365c54, InitParams jobject) → StartLuaAppDM
   (0x1023efe2c) → V2StartAppWithParams (0x10258b144, StartAppParams jobject),
   then the V1 AppStart__ fallback (0x102338510). All params are AutoValue JNI
   jobjects (new_fake_object), serviced by the getter shim — NOT JSON strings.

## Empirical result

On the stable productized recipe (render chain keeps the client alive to exit
124), the ladder runs clean:

```
[elfjit:v2boot] after boot start: [0x106829ea8] = 0x0
[elfjit:v2boot] driving nativeGameGlobalInit @ guest 0x102206404 ...
```

`nativeGameGlobalInit` **executes real engine code** (block cache grows to 2192,
far above the ~434 idle baseline) and the process stays stable — exit 124, real
triangle + textured-quad renders, persist roundtrip byte-exact, ZERO
SIGSEGV/SIGABRT, **no json-string-length-overflow** (the abort did NOT fire even
though GlobalInit ran). The per-window JIT_FRAMEWORK_DUMP samples show
`task-v4 [0x106829ea8]=0x0` **the entire window** while GlobalInit drives.

**Interpretation (honest):** driving the in-order ladder headlessly does NOT
populate the type-4 producer vector — it stays 0 through the real `nativeGameGlobalInit`
execution. This corroborates SH53's static disproof at runtime: the recon's
"TaskScheduler init installs the vector in-image once the ladder runs" is not
reproduced here. One caveat: `nativeGameGlobalInit` itself does not return
within the run window (it advances and parks mid-setup, so the later rungs —
V2InitWithParams onward — are not reached in this single-shot sequential drive;
only rung 1 is observed). So the vector staying 0 is measured DURING GlobalInit's
execution, not after a full ordered completion that reaches the TaskScheduler
install site.

**Standing structural wall (unchanged, now with a runtime data point):** the
type-4 producer vector `[0x106829ea8]` is framework-glue-seeded only; headless
in-order execution of the real GlobalInit does not populate it. The sustainable
host-side seed lever (`--taskv4-seed` + `--deque-node-live`, SH49) remains the
only mechanism proven to make the dispatch plane run.