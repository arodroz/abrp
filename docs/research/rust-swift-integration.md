# Rust ↔ Swift integration options for ABRP-native

Research for issue #8. Question: which mechanism should ABRP-native use to call a Rust
core (Routing Engine, Energy Model, Charging Stop search) from a Swift/SwiftUI iOS app,
and where should the Rust boundary sit? Compared: **UniFFI** (Mozilla), **swift-bridge**,
**cargo-swift / hand-built XCFramework** packaging, and a **hand-written C ABI via cbindgen**.
Every claim carries the URL it was checked against (fetched 2026-08-27).

## TL;DR

- All four options end up as the same physical artifact: a Rust **static library** compiled
  for `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios`, bundled with C headers
  and a `module.modulemap` into an **XCFramework**, consumed by SwiftPM as a `binaryTarget`.
  The generators differ only in what glue sits above that C layer.
- **UniFFI** is the mainstream choice for Rust-core mobile SDKs (Mozilla application-services,
  Ferrostar). It gives idiomatic Swift (structs, enums, `throws`, `async`) at the price of
  serialising every non-primitive argument through a `RustBuffer`, no built-in cancellation,
  and a still-open Swift 6 strict-concurrency issue for async/callback code.
- **swift-bridge** is zero-copy and has bidirectional async, but is a one-maintainer project
  with an admittedly incomplete book; async support was built with a `tokio` feature.
- **cbindgen + hand-written C ABI** is what Signal's libsignal and (in spirit) 1Password use:
  maximum control (tokio runtime owned by Rust, Swift `Task` cancellation forwarded to Rust),
  maximum boilerplate.
- Recommendation for ABRP-native: **UniFFI in library mode, coarse-grained boundary** — call
  Rust with a whole planning request and get back a whole `Plan`; keep GPS, networking, map
  rendering and all SwiftUI state on the Swift side (exactly the Ferrostar split). Details in §8.

## 1. What each option is

### UniFFI

Multi-language bindings generator from Mozilla. For Swift it emits three things: a C header
with the FFI structs/functions, a modulemap defining a Swift module for those C declarations,
and a Swift source file "that defines the Swift API used by consumers. This imports the FFI
module." (https://mozilla.github.io/uniffi-rs/latest/swift/overview.html)

Latest release at time of writing: **v0.32.0, 2026-06-30**
(https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md).

Interfaces are declared either in a `.udl` file or with proc-macros (`#[uniffi::export]`,
`#[derive(uniffi::Record)]`, `#[derive(uniffi::Object)]`); "library mode"
(`uniffi-bindgen generate --library <cdylib>`) reads the metadata out of the compiled library
and "should be preferred where possible"
(https://mozilla.github.io/uniffi-rs/latest/tutorial/foreign_language_bindings.html).
Since v0.28.2 there is a dedicated `uniffi-bindgen-swift` binary with Swift-specific flags
(https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md).

### swift-bridge

"swift-bridge generates bindings for calling Rust from Swift and vice versa" via a
`#[swift_bridge::bridge]` module that declares `extern "Rust"` / `extern "Swift"` blocks.
The README stresses that "none of its generated FFI code uses object serialization, cloning,
synchronization or any other form of unnecessary overhead." Repository stats at fetch time:
~1.1k stars, 412 commits, 92 open issues, targets Swift 6.0+
(https://github.com/chinedufn/swift-bridge). The book states plainly: "The `swift-bridge`
book is a work-in-progress with many chapter either sparse or empty"
(https://chinedufn.github.io/swift-bridge/).

### cargo-swift

A Cargo plugin that wraps UniFFI: `cargo swift init` scaffolds a UniFFI crate, `cargo swift
package` builds the Rust library and bundles it as a Swift package. macOS-only. It cannot
detect the project's UniFFI version, so the plugin version must be matched manually
(compatibility table maps UniFFI 0.25–0.31.1 to cargo-swift 0.5–0.11.1)
(https://github.com/antoniusnaumann/cargo-swift). It is a packaging convenience, not a
distinct binding technology.

### cbindgen + hand-written C ABI

cbindgen "creates C/C++11 headers for Rust libraries which expose a public C API"; it is run
as a CLI or from `build.rs` with a `cbindgen.toml`. The README warns that "cbindgen may
randomly fail to support some particular situation simply because no one has put in the effort
to handle it yet" (https://github.com/mozilla/cbindgen). Everything above the C layer
(ownership, strings, errors, async) is written by hand on both sides.

## 2. Ergonomics and type mapping

| | UniFFI | swift-bridge | cbindgen / C ABI |
|---|---|---|---|
| Primitives | `u32`→`UInt32`, `String`→`String`, `bool`→`Bool` (https://mozilla.github.io/uniffi-rs/latest/swift/overview.html) | Primitives, `String`/`&str`↔`String`/`RustStr` (https://chinedufn.github.io/swift-bridge/) | C ints, `*const c_char`; Swift sees `OpaquePointer`, `UnsafePointer` (https://www.swift.org/documentation/articles/wrapping-c-cpp-library-in-swift.html) |
| Collections / optionals | `Vec<T>`→`sequence<T>`→`[T]`, `HashMap<K,V>`→`record<K,V>`, `Option<T>`→`T?` (https://mozilla.github.io/uniffi-rs/latest/types/builtin_types.html) | `Vec<T>`↔`RustVec<T>`, `Option`↔`Optional`, tuples, `SwiftArray<T>` (https://github.com/chinedufn/swift-bridge) | Hand-rolled (pointer + length) |
| Records / enums | Records→Swift structs, enums→Swift enums with associated values; records can be `let`-only via `generate_immutable_records`, `Codable`/`CaseIterable` conformance opt-in (https://mozilla.github.io/uniffi-rs/latest/swift/configuration.html) | "transparent" structs/enums copied field-by-field; "opaque" types held by pointer (https://github.com/chinedufn/swift-bridge) | `#[repr(C)]` structs only; anything with `String`/`Vec` needs manual layout |
| Objects | `Arc<T>` heap objects → Swift protocol + class; must be `Send + Sync`, methods take `&self` so mutation needs interior mutability (https://mozilla.github.io/uniffi-rs/latest/types/interfaces.html) | Opaque Rust types owned by Swift class wrapper | Opaque handle + explicit `free` |
| Errors | Rust `Result<T, E>` with an `Error` enum → Swift `throws`; an interface/`dyn Trait` may also be an error type (https://mozilla.github.io/uniffi-rs/latest/types/errors.html) | `Result<T,E>`↔`RustResult<T,E>` (https://chinedufn.github.io/swift-bridge/) | Error codes / out-params by hand |
| Swift→Rust callbacks | "Foreign traits" (Swift class implements a Rust trait, passed as `Arc<dyn Trait>`); older "callback interfaces" are soft-deprecated (https://mozilla.github.io/uniffi-rs/latest/types/callback_interfaces.html) | `extern "Swift"` block; async Swift fns need typed `throws(E)` (Swift 5.9+) (https://github.com/chinedufn/swift-bridge/blob/master/book/src/bridge-module/functions/README.md) | Function pointers + `void*` context |

Mechanism and cost: in UniFFI "Non-trivial types such as Strings, Optionals and Records, etc.
are lowered to a byte buffer called a `RustBuffer` internally", using "an ad-hoc fixed-width
format which is designed mainly for simplicity"
(https://mozilla.github.io/uniffi-rs/latest/internals/lifting_and_lowering.html). v0.32.0 added
zero-copy `&[u8]`/`&mut [u8]` (`ForeignBytes`, Swift `inout Data`) for byte arguments
(https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md). Consequence for ABRP: a `Plan`
with thousands of polyline points and a per-point SoC curve will be serialised once per call —
cheap if the call is coarse (one call per planning run), expensive if the boundary is chatty.

Field reports: Ferrostar says UniFFI-generated APIs "truly feel idiomatic" and the one pain is
that "you can't have mutable references to `self` in UniFFI interfaces and must rely on interior
mutability patterns instead" — state must be `Send + Sync`, i.e. "atomics and mutexes"
(https://stadiamaps.com/blog/ferrostar-building-a-cross-platform-navigation-sdk-in-rust-part-1/).
LY Corporation contrasts manual bridging ("a lot of boilerplate code to bridge the gaps,
including the C code to wrap Rust functions, the bridging headers for Swift") with UniFFI's
`#[uniffi::export]` and chose UniFFI (https://techblog.lycorp.co.jp/en/20241002a).

## 3. Async and concurrency across the boundary

### UniFFI

- "It can convert a Rust `Future`/`async fn` to and from foreign native futures (`async`/`await`
  in Python/Swift/Ruby, `suspend fun` in Kotlin etc.)". "In Rust `Future` terminology this means
  the foreign bindings supply the 'executor' … There's no requirement for a Rust event loop."
  (https://mozilla.github.io/uniffi-rs/latest/futures.html)
- Internals: Swift calls `rust_future_poll` with a continuation callback; "If the future is
  pending, then the generated code registers a waker that will call the callback function with
  `RUST_FUTURE_WAKE`", and "The async Rust code runs in a `Future::poll` method, inside the
  foreign event loop." Crates that need their own runtime are flagged: "some Rust crates will
  present async APIs that silently start up runtimes in the background. For example, reqwest
  will start a tokio runtime." A `uniffi::async_runtime` attribute exists "but it's not clear if
  we want to continue to support it."
  (https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/internals/async-overview.md;
  FFI surface: https://docs.rs/uniffi_core/latest/uniffi_core/ffi/rustfuture/index.html)
- **No cancellation**: "We don't directly support cancellation in UniFFI even when the underlying
  platforms do. You should build your cancellation in a separate, library specific channel; for
  example, exposing a `cancel()` method that sets a flag that the library checks periodically …
  There's no builtin way to cancel a future, nor to cause/raise a platform native async
  cancellation error (eg, a swift `CancellationError`)."
  (https://mozilla.github.io/uniffi-rs/latest/futures.html)
- Trait interfaces can have async methods, Rust- or foreign-implemented (CHANGELOG,
  https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md). Whether async Rust can `await`
  an async Swift implementation is asked in issue #2633 (opened 2025-09-02) and has no answer
  (https://github.com/mozilla/uniffi-rs/issues/2633).
- **Swift 6 strict concurrency**: "UniFFI has partial support for Swift 6 … Most generated code
  will conform to `Sendable`. Depending on your Swift compiler options, you may find rough edges
  where this support doesn't quite exist. At time of writing, it is known that async code will
  not conform." Tracked in #2448, which is **open**; "traits and callbacks are problematic"
  (https://mozilla.github.io/uniffi-rs/latest/swift/overview.html,
  https://github.com/mozilla/uniffi-rs/issues/2448). The originating report #2274 shows the
  symptom: awaiting an async method on a `#[derive(uniffi::Object)]` class from an isolated
  context yields "Sending 'self'-isolated value … risks causing data races"
  (https://github.com/mozilla/uniffi-rs/issues/2274). Since v0.29.x, interfaces and generated
  protocols are marked `Sendable`, so foreign-trait implementations must be `Sendable` too
  (https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md).
- Swift Forums guidance on the underlying question: "Swift's `Sendable` corresponds to Rust's
  `Send`"; a wrapper class should be `Sendable` only if the Rust type is `Sync`, and blanket
  `@unchecked Sendable` is discouraged
  (https://forums.swift.org/t/question-on-sendability-swift-6-data-race-safety-and-ffi-interfaces/76219).

### swift-bridge

Async in both directions; "As a starting point, we're using Swift's built-in async runtime to
drive our async functions." The `async` feature "pulls in tokio and once_cell as dependencies";
the initial PR did "not yet support arguments in async Rust functions"
(https://github.com/chinedufn/swift-bridge/pull/31). Today's book shows
`async fn load_user(url: &str) -> Result<User, ApiError>` awaited from Swift and async Swift
functions callable from Rust via generated "callback-based FFI"
(https://github.com/chinedufn/swift-bridge/blob/master/book/src/bridge-module/functions/README.md).

### Hand-written C ABI (libsignal precedent)

Signal owns the runtime: Rust builds a multi-thread tokio runtime
(`enable_io().enable_time().thread_name("libsignal-tokio-worker")`), spawns each bridged future
with a `CancellationId` stored in a task map, and reports the result on a blocking thread
(https://raw.githubusercontent.com/signalapp/libsignal/main/rust/bridge/shared/types/src/net/tokio.rs).
Swift wraps every call in `withTaskCancellationHandler`; `onCancel` calls
`signal_tokio_async_context_cancel`, so **Swift `Task` cancellation propagates into Rust**
(https://raw.githubusercontent.com/signalapp/libsignal/main/swift/Sources/LibSignalClient/TokioAsyncContext.swift,
https://raw.githubusercontent.com/signalapp/libsignal/main/swift/Sources/LibSignalClient/Net.swift).
This is the only approach among the four that gives structured-concurrency-correct cancellation
today — at the cost of writing the promise/handle plumbing by hand.

### What this means for a CPU-bound Routing Engine

Route search and Energy Model evaluation are CPU-bound, not I/O-bound. None of the
generator-provided async mechanisms make CPU work off-thread by themselves; UniFFI's async
runs `poll` "inside the foreign event loop". The practical pattern (used by Ferrostar) is
**synchronous Rust calls invoked from a Swift background context**: FerrostarCore calls
`navigationSession?.updateUserLocation(...)` synchronously and dispatches heavy work with
`DispatchQueue.global(qos: .default).async`, publishing results back on main
(https://raw.githubusercontent.com/stadiamaps/ferrostar/main/apple/Sources/FerrostarCore/FerrostarCore.swift).
For ABRP this means: `Task.detached { try plannerHandle.plan(request) }` around a *blocking*
Rust function, plus a Rust-side `cancel()` flag polled inside the search loop, per UniFFI's own
advice. Rayon inside the Rust call is fine; tokio is unnecessary unless Rust does I/O.

## 4. Binary size

- No option is materially smaller than another; size is dominated by the Rust code and the
  Cargo profile, not the glue. Numbers from bitdrift's Rust mobile SDK (iOS): compile flags
  (`opt-level=s`, `codegen-units=1`) −26 %, debug-info stripping −25 %, LTO + dead-code
  stripping −90 %, `panic=abort` −19 %, custom stdlib −15 %; final SDK ≈ 1 MB compressed. Flags:
  `-Ccodegen-units=1 -Copt-level=s -Cpanic=abort -Clto=fat -Cembed-bitcode`. Caveat: `panic=abort`
  forces "making heavy use of Result-types everywhere, aggressive clippy linting"
  (https://blog.bitdrift.io/post/optimizing-rust-mobile-sdk-binary-size).
- Counter-example of an unoptimised build: LY Corporation's demo shipped a 25 MB Rust library
  in a 130 KB app (https://techblog.lycorp.co.jp/en/20241002a).
- The Rust Performance Book and min-sized-rust document the same knobs (`lto`, `opt-level`,
  `codegen-units`, `panic`, `strip`)
  (https://nnethercote.github.io/perf-book/build-configuration.html,
  https://github.com/johnthagen/min-sized-rust).
- UniFFI adds a small runtime (RustBuffer, checksums — checksum tests can be omitted with
  `omit_checksums`) (https://mozilla.github.io/uniffi-rs/latest/swift/configuration.html).
  swift-bridge's `async` feature adds tokio (https://github.com/chinedufn/swift-bridge/pull/31).
  A hand-written C ABI adds nothing beyond what you write.
- Static linking (the norm in every precedent below) lets the Apple linker dead-strip unused
  Rust symbols into the app binary; there is no separate dylib to ship.

## 5. Build complexity in Xcode / SwiftPM

The pipeline is the same regardless of generator; the precedents converge on one script:

1. `cargo build --lib --release --target {x86_64-apple-ios, aarch64-apple-ios-sim, aarch64-apple-ios}`.
2. Generate bindings, e.g. `uniffi-bindgen-swift <lib.a> <staging> --swift-sources --headers
   --modulemap --module-name FooFFI --modulemap-filename module.modulemap`.
3. `lipo -create` the two simulator `.a` files into one fat simulator library.
4. `xcodebuild -create-xcframework -library <device.a> -headers <staging> -library <sim-fat.a>
   -headers <staging> -output libfoo-rs.xcframework`.
5. `ditto -c -k … .zip` and `swift package compute-checksum`, then `.binaryTarget(name:url:checksum:)`.
   (https://stadiamaps.com/blog/ferrostar-building-a-cross-platform-navigation-sdk-in-rust-part-2/)

Facts that bite:

- The modulemap "must be renamed to `module.modulemap`, which is the default value expected by
  Clang and XCFrameworks for exposing the C FFI library to Swift"
  (https://mozilla.github.io/uniffi-rs/latest/swift/module.html).
- SwiftPM binary targets are Apple-only and XCFramework-only; a checksum is mandatory for remote
  artifacts so that "an attacker needs to compromise both the server which provides the artifact
  as well as the git repository" (SE-0272,
  https://github.com/apple/swift-evolution/blob/main/proposals/0272-swiftpm-binary-dependencies.md;
  Apple reference: https://developer.apple.com/documentation/xcode/distributing-binary-frameworks-as-swift-packages,
  https://developer.apple.com/documentation/xcode/creating-a-multi-platform-binary-framework-bundle).
- Ferrostar: "we haven't found a way to use the same Package.swift unmodified for both local
  development within the 'project' and for publishing" → a `useLocalFramework` toggle switching
  between `path:` and `url:` (https://stadiamaps.com/blog/ferrostar-building-a-cross-platform-navigation-sdk-in-rust-part-2/;
  current manifest: `ferrostarFFI` binary target, iOS 16+,
  https://raw.githubusercontent.com/stadiamaps/ferrostar/main/Package.swift).
- Mozilla's design note: the XCFramework "contains the compiled Rust code for all the crates
  listed in Cargo.toml as a static library" plus headers and modulemaps; hand-building it "does
  risk diverging from the expected format if Apple changes the details of xcframeworks in future
  Xcode releases" (https://mozilla.github.io/application-services/book/design/swift-package-manager.html).
  Their `build-xcframework.sh` builds the same three targets, lipo-merges the simulator slices,
  and defaults `IOS_DEPLOYMENT_TARGET` to 15.0
  (https://github.com/mozilla/application-services/blob/main/megazords/ios-rust/build-xcframework.sh).
- Alternative for an in-repo app (no package publishing): UniFFI's Xcode-integration page
  describes a build phase that compiles the crate to a static lib, a build rule that runs
  `uniffi-bindgen` on the `.udl`, and adding the generated header as a **Public** header
  (https://mozilla.github.io/uniffi-rs/latest/swift/xcode.html).
- swift-bridge's equivalent is `swift-bridge-cli create-package --bridges-dir … --ios --simulator
  --name …`, producing a package with the generated Swift/C and per-platform static libs
  (https://chinedufn.github.io/swift-bridge/building/swift-packages/index.html); its build API
  writes one concatenated `{crate}.h` and `{crate}.swift`
  (https://github.com/chinedufn/swift-bridge/blob/master/crates/swift-bridge-build/src/lib.rs).
- Ferrostar's honest summary: "The process of generating bindings for Swift and Kotlin,
  integrating this into your build tooling, and packaging everything into a usable SPM / Maven
  package is also quite complex", and UniFFI "is evolving quite rapidly" with breaking changes
  (https://stadiamaps.com/blog/ferrostar-building-a-cross-platform-navigation-sdk-in-rust-part-1/).
  cargo-swift or the `uniffi-starter` template (`build-ios.sh`, a `Package.swift` that
  "documents the UniFFI setup (which is... special thanks to SPM quirks)") remove most of it
  (https://github.com/antoniusnaumann/cargo-swift, https://github.com/ianthetechie/uniffi-starter).

## 6. SwiftUI-friendliness

- UniFFI records are plain Swift structs (optionally `let`-only and `Codable`), enums are Swift
  enums, so they can be stored in `@Observable`/`@Published` state and diffed by SwiftUI directly
  (https://mozilla.github.io/uniffi-rs/latest/swift/configuration.html). Equality/hash/ordering
  can be exported from Rust traits (`Eq`, `Cmp`; fixed in v0.32.0 for odd capitalisation)
  (https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md).
- Objects are reference types (classes) backed by `Arc`; they are not value-diffable and must be
  wrapped in a Swift model object. Ferrostar's pattern: a Swift `FerrostarCore` class holding
  `@Published public private(set) var state: NavigationState?` and calling the Rust object
  synchronously, with a TODO to make it an actor "so we can verify that we've published things
  back on the main actor"
  (https://raw.githubusercontent.com/stadiamaps/ferrostar/main/apple/Sources/FerrostarCore/FerrostarCore.swift).
  Ferrostar ships separate `FerrostarCore`, `FerrostarSwiftUI`, `FerrostarMapLibreUI`,
  `FerrostarCarPlayUI` targets (https://raw.githubusercontent.com/stadiamaps/ferrostar/main/Package.swift).
- swift-bridge "transparent" structs are likewise value types; opaque types are pointer-owning
  classes (https://github.com/chinedufn/swift-bridge).
- With a raw C ABI, nothing is SwiftUI-ready: every struct must be converted by hand (1Password
  solves this with Typeshare, which generates Swift types from `serde`-annotated Rust and moves
  data as serialized payloads over the FFI — "seamless synchronization of your shared data types
  across languages" (https://1password.com/blog/typeshare-for-rust,
  https://github.com/1Password/typeshare, https://serokell.io/blog/rust-in-production-1password)).

## 7. Debugging

- Rust emits dSYM-compatible DWARF; with the Rust sources added to the Xcode project, "you can
  set breakpoints upon lines and your run target will respect them" when the Rust lib is built
  in a debug profile (https://gist.github.com/shirakaba/afc1c6f212892c43039dbe056c604619).
  Cargo places the `.dSYM` under `target/<profile>/deps/`, which LLDB may not find automatically
  (https://github.com/rust-lang/cargo/issues/4056).
- UniFFI-specific: v0.31.1 "Fixed iOS crash when address sanitizer is enabled" and "Fixed memory
  link in async code" (https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md) — ASan runs
  are possible but were fragile until recently. Rust `Result` errors surface as Swift `throws`
  (https://mozilla.github.io/uniffi-rs/latest/types/errors.html), so the normal Swift error path
  is debuggable; panics must be avoided with `panic=abort` builds (see §4).
- With a hand-written C ABI, stack traces cross the boundary with one plain C frame; with
  UniFFI/swift-bridge there are 2–4 generated frames (lift/lower, continuation) between the
  Swift call and the Rust body.

## 8. Precedents

| Project | Mechanism | Boundary drawn | Source |
|---|---|---|---|
| Mozilla application-services / Firefox iOS | UniFFI; megazord crate → static-lib XCFramework → SwiftPM binary target (older `rust-components-swift` repo is now deprecated in favour of direct releases) | Storage, sync, FxA, experiments in Rust; UI in Swift | https://github.com/mozilla/application-services, https://mozilla.github.io/application-services/book/design/swift-package-manager.html, https://github.com/mozilla/rust-components-swift |
| Ferrostar (Stadia Maps) navigation SDK | UniFFI, XCFramework + SwiftPM, iOS 16+ | Rust: data models, Valhalla/OSRM request/response parsing, "Spatial algorithms like line snapping and distance calculations", "Navigation state machine". Swift: "Platform-native UI (SwiftUI …)", sensors, networking. Hexagonal: "you can't skip across multiple layer boundaries" | https://stadiamaps.github.io/ferrostar/architecture.html |
| Signal libsignal | cbindgen C headers, `build_ffi.sh` static libs for the three iOS targets; CocoaPods canonical, SwiftPM "for local development only"; hand-written Swift with tokio context and cancellation | Crypto and network protocol in Rust | https://raw.githubusercontent.com/signalapp/libsignal/main/swift/README.md, https://github.com/signalapp/libsignal |
| 1Password | Custom FFI + Typeshare-generated types (serde) | Business logic and security core in Rust, native UIs | https://1password.com/blog/typeshare-for-rust, https://serokell.io/blog/rust-in-production-1password |
| LY Corporation | UniFFI | Crypto (AES 10 M iterations: Rust ≈15 s vs CryptoKit ≈22 s on iPhone 11 Pro); noted Swift Concurrency support lagged UniFFI until 2023 | https://techblog.lycorp.co.jp/en/20241002a |

Notably, Ferrostar's Rust core already talks to Valhalla/OSRM — the same two engines ABRP's
attribution screen credits (`docs/abrp-tech-stack.md`).

## 9. Facts to draw the Rust boundary for ABRP-native

1. **Coarse calls only.** UniFFI serialises every record/sequence through a `RustBuffer`
   (§2). Design the API as `plan(request: PlanRequest) throws -> Plan` and
   `energy(for leg: LegInput, vehicle: VehicleModel) -> LegEnergy`, never per-edge or per-point
   calls. Return the SoC curve as a `Vec<f32>` (one buffer), not an object per sample.
2. **Rust owns: Routing Engine, Energy Model, Charging Curve maths, Charging Stop search.**
   These are pure, CPU-bound functions over a road graph, a Vehicle Model and a Charger set —
   the Ferrostar split. Swift owns: GPS/CoreLocation, URLSession networking (Charger data, tiles,
   weather), MapKit/MapLibre rendering, all SwiftUI state. Ferrostar and LY both note that mobile
   networking is better left native (§8; https://techblog.lycorp.co.jp/en/20241002a).
3. **Synchronous Rust + Swift `Task.detached` for planning**, not UniFFI async (§3). Add an
   explicit `cancel()` on the planner object (atomic flag polled in the search loop) because
   UniFFI has no cancellation and Swift 6 strict concurrency for UniFFI async is still open (#2448).
4. **Planner state as one `uniffi::Object` with interior mutability** (`Mutex`/`RwLock` around
   the loaded graph and Charger index), constructed once and reused; it must be `Send + Sync`.
5. **Records must be value types**: `Plan`, `Leg`, `ChargingStop`, `Charger`, `VehicleModel` as
   `#[derive(uniffi::Record)]` with `generate_immutable_records = true` so SwiftUI can diff them.
6. **Errors as a single `#[derive(uniffi::Error)]` enum** (`NoRouteFound`, `InsufficientRange`,
   `Cancelled`, …) surfacing as Swift `throws`; build with `panic = "abort"` and treat any panic
   as a bug.
7. **Packaging**: library-mode `uniffi-bindgen-swift`, static XCFramework, local `path:` binary
   target during development (Ferrostar's `useLocalFramework` pattern), `cargo-swift` or a copy of
   Ferrostar's/uniffi-starter's script for the three targets. Pin the UniFFI version in both
   `Cargo.toml` and the bindgen invocation; expect breaking changes on upgrade.
8. **Size profile from day one**: `opt-level = "s"`, `lto = "fat"`, `codegen-units = 1`,
   `panic = "abort"`, `strip = true` (§4). Avoid pulling tokio/reqwest into the core crate; no
   I/O in Rust removes the need for any Rust runtime.
9. **Fallbacks if UniFFI's overhead ever shows in profiles**: keep the C-layer contract (static
   lib + modulemap) and swap in hand-written `extern "C"` entry points for the hot path (e.g.
   passing the road graph as `&[u8]` — zero-copy in UniFFI ≥0.32 anyway) rather than switching
   generator wholesale. swift-bridge is not recommended as the primary mechanism given its
   single-maintainer status and incomplete documentation (§1).

## Open questions

- Whether Swift 6 `-strict-concurrency=complete` will be enforced app-wide; if so, verify the
  generated UniFFI code compiles under it for the exact object/trait set used (issue #2448).
- Whether the road graph lives in Rust memory (loaded once from a file path passed at init) or
  is streamed from Swift — affects whether zero-copy `&[u8]` arguments matter.
- Real binary-size measurement of a stub crate with the §4 profile on an iOS device build.
