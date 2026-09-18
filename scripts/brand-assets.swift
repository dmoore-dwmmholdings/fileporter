// Derives the app's brand assets from the branding kit. The kit ships raster
// art only, so everything here is a crop or a resample of its masters — no
// redrawing, which is what the kit asks for.
//
//   swift scripts/brand-assets.swift <repo root>
//
// Writes: the in-app mark (transparent, trimmed to its content) for both apps,
// and every app-icon size the desktop bundle names. The iOS icon, the macOS
// .icns and the Windows .ico are copied from the kit as-is.
import AppKit

let root = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? ".")
let kit = root.appending(path: "branding/fileporter-file-portal")

func load(_ path: String) -> CGImage {
    guard
        let data = try? Data(contentsOf: kit.appending(path: path)),
        let source = CGImageSourceCreateWithData(data as CFData, nil),
        let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else {
        FileHandle.standardError.write(Data("cannot read \(path)\n".utf8))
        exit(1)
    }
    return image
}

func write(_ image: CGImage, to url: URL) {
    try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else { exit(1) }
    CGImageDestinationAddImage(destination, image, nil)
    CGImageDestinationFinalize(destination)
}

/// Draws `image` into a `size`-wide bitmap, keeping transparency.
func resample(_ image: CGImage, width: Int, height: Int) -> CGImage {
    let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    return context.makeImage()!
}

/// The visible bounds of the symbol, so the mark is not padded by the kit's
/// own icon margins when it sits beside a word in a header.
func contentBox(_ image: CGImage) -> CGRect {
    let width = image.width, height = image.height
    var pixels = [UInt8](repeating: 0, count: width * height * 4)
    pixels.withUnsafeMutableBytes { buffer in
        let context = CGContext(
            data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    }
    var minX = width, minY = height, maxX = -1, maxY = -1
    for y in 0..<height {
        for x in 0..<width where pixels[(y * width + x) * 4 + 3] > 8 {
            minX = min(minX, x); maxX = max(maxX, x)
            minY = min(minY, y); maxY = max(maxY, y)
        }
    }
    guard maxX >= minX, maxY >= minY else { return CGRect(x: 0, y: 0, width: width, height: height) }
    return CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1)
}

// ── The in-app mark ──────────────────────────────────────────────────────
let symbol = load("source/symbol-generated.png")
let trimmed = symbol.cropping(to: contentBox(symbol))!
let aspect = Double(trimmed.width) / Double(trimmed.height)

func mark(height: Int) -> CGImage {
    resample(trimmed, width: Int((Double(height) * aspect).rounded()), height: height)
}

// The desktop header draws it at 18px; ship 3x for high-DPI displays.
write(mark(height: 72), to: root.appending(path: "src/assets/brand-mark.png"))
for (scale, suffix) in [(1, ""), (2, "@2x"), (3, "@3x")] {
    write(
        mark(height: 24 * scale),
        to: root.appending(path: "ios/Fileporter/Assets.xcassets/BrandMark.imageset/brand-mark\(suffix).png"))
}

// ── App icons ────────────────────────────────────────────────────────────
let master = load("icons/png/fileporter-1024.png")
write(master, to: root.appending(path: "ios/Fileporter/Assets.xcassets/AppIcon.appiconset/AppIcon.png"))

let desktopSizes: [(Int, String)] = [
    (32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png"), (512, "icon.png"), (64, "64x64.png"),
    (30, "Square30x30Logo.png"), (44, "Square44x44Logo.png"), (71, "Square71x71Logo.png"),
    (89, "Square89x89Logo.png"), (107, "Square107x107Logo.png"), (142, "Square142x142Logo.png"),
    (150, "Square150x150Logo.png"), (284, "Square284x284Logo.png"), (310, "Square310x310Logo.png"),
    (50, "StoreLogo.png"),
]
for (size, name) in desktopSizes {
    write(resample(master, width: size, height: size), to: root.appending(path: "src-tauri/icons/\(name)"))
}

// The kit's own platform containers, copied rather than rebuilt.
for (from, to) in [
    ("icons/macos/Fileporter.icns", "src-tauri/icons/icon.icns"),
    ("icons/windows/Fileporter.ico", "src-tauri/icons/icon.ico"),
    ("web/favicon.ico", "public/favicon.ico"),
    ("web/apple-touch-icon.png", "public/apple-touch-icon.png"),
] {
    let destination = root.appending(path: to)
    try? FileManager.default.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? FileManager.default.removeItem(at: destination)
    try? FileManager.default.copyItem(at: kit.appending(path: from), to: destination)
}

print("brand assets written from \(kit.lastPathComponent)")
