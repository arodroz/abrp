// swift-tools-version:5.10
// PlannerKit: the Swift-facing wrapper around the `planner_ffi` xcframework
// (wayfinder #34, ADR 0004). `planner_ffi` is a local binary target built by
// `scripts/build-xcframework.sh`; PlannerKit itself is what the app links.
import PackageDescription

let package = Package(
    name: "PlannerKit",
    platforms: [.iOS(.v16), .macOS(.v13)],
    products: [
        .library(name: "PlannerKit", targets: ["PlannerKit"])
    ],
    targets: [
        .binaryTarget(name: "planner_ffi", path: "artifacts/planner_ffi.xcframework"),
        .target(
            name: "PlannerKit",
            dependencies: ["planner_ffi"],
            swiftSettings: [.enableExperimentalFeature("StrictConcurrency")]
        ),
        .testTarget(
            name: "PlannerKitTests",
            dependencies: ["PlannerKit"],
            swiftSettings: [.enableExperimentalFeature("StrictConcurrency")]
        ),
    ]
)
