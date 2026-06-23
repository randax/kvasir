// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "KvasirViewer",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(
            name: "KvasirViewer",
            targets: ["KvasirViewer"]
        ),
        .library(
            name: "KvasirViewerCore",
            targets: ["KvasirViewerCore"]
        )
    ],
    targets: [
        .executableTarget(
            name: "KvasirViewer",
            dependencies: ["KvasirViewerCore"]
        ),
        .target(
            name: "KvasirViewerCore"
        ),
        .testTarget(
            name: "KvasirViewerCoreTests",
            dependencies: ["KvasirViewerCore"]
        ),
        .testTarget(
            name: "KvasirViewerTests",
            dependencies: ["KvasirViewer"]
        )
    ]
)
