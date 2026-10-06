// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "L6Kit",
    platforms: [.iOS(.v18), .macOS(.v15)],
    products: [
        .library(name: "L6Kit", targets: ["L6Kit"]),
    ],
    targets: [
        .target(name: "L6Kit"),
        .testTarget(name: "L6KitTests", dependencies: ["L6Kit"]),
    ],
    swiftLanguageModes: [.v6]
)
