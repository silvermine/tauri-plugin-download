// swift-tools-version: 5.5
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "tauri-plugin-download",
    platforms: [
        .macOS(.v10_15),
        .iOS(.v15)
    ],
    products: [
        // Products define the executables and libraries a package produces, and make them visible to other packages.
        .library(
            name: "tauri-plugin-download",
            type: .static,
            targets: ["tauri-plugin-download"]),
    ],
    dependencies: [
        .package(name: "DownloadManagerKit", path: "DownloadManagerKit"),
        .package(name: "Tauri", path: "../.tauri/tauri-api")
    ],
    targets: [
        // Targets are the basic building blocks of a package. A target can define a module or a test suite.
        // Targets can depend on other targets in this package, and on products in packages this package depends on.
        .target(
            name: "tauri-plugin-download",
            dependencies: [
                .byName(name: "DownloadManagerKit"),
                .byName(name: "Tauri")
            ],
            path: "Sources")
    ]
)
