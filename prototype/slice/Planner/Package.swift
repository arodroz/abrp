// swift-tools-version:5.5
// The swift-tools-version declares the minimum version of Swift required to build this package.
// Swift Package: Planner

import PackageDescription;

let package = Package(
    name: "Planner",
    platforms: [
        .macOS(.v10_15), .iOS(.v13)
    ],
    products: [
        .library(
            name: "Planner",
            targets: ["Planner"]
        )
    ],
    dependencies: [ ],
    targets: [
        .binaryTarget(name: "plannerFFI", path: "./plannerFFI.xcframework"),
        .target(
            name: "Planner",
            dependencies: [
                .target(name: "plannerFFI")
            ]
        ),
    ]
)