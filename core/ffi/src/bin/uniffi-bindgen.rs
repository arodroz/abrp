//! Bindgen CLI entry point (ADR 0004 point 1: library mode, no UDL). Built
//! only with `--features cli`; `scripts/build-xcframework.sh` runs it
//! against the built `libplanner_ffi` to generate the Swift bindings.

fn main() {
    uniffi::uniffi_bindgen_main();
}
