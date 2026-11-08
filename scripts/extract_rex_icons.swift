// Run on macOS: swift scripts/extract_rex_icons.swift BUNDLE OUTPUT_DIRECTORY
import AppKit

let args = CommandLine.arguments
precondition(args.count == 3, "Expected the Rex SuperBlockIcons bundle and output directory")
let bundleURL = URL(fileURLWithPath: args[1])
guard let bundle = Bundle(url: bundleURL) else { fatalError("Cannot open Rex icon bundle") }
let task = Process()
let output = Pipe()
task.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
task.arguments = ["assetutil", "--info", bundleURL.appendingPathComponent("Contents/Resources/Assets.car").path]
task.standardOutput = output
try task.run()
let data = output.fileHandleForReading.readDataToEndOfFile()
task.waitUntilExit()
precondition(task.terminationStatus == 0, "Cannot read Rex asset inventory")
let inventory = try JSONSerialization.jsonObject(with: data) as! [[String: Any]]
let names = Set(inventory.compactMap { $0["Name"] as? String }.filter { $0.hasPrefix("block-glyph-") }).sorted()
precondition(names.contains("block-glyph-pi") && names.contains("block-glyph-zsh"), "Missing expected Rex program icons")
let destination = URL(fileURLWithPath: args[2], isDirectory: true)
try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
for name in names {
    guard let image = bundle.image(forResource: NSImage.Name(name)) else { fatalError("Missing icon: \(name)") }
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 48, pixelsHigh: 48, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    image.draw(in: NSRect(x: 0, y: 0, width: 48, height: 48), from: .zero, operation: .copy, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    let pixels = bitmap.bitmapData!
    var weightedLightness = 0.0
    var alphaSum = 0.0
    for offset in stride(from: 0, to: 48 * 48 * 4, by: 4) {
        let alpha = Double(pixels[offset + 3]) / 255
        let lightness = (0.2126 * Double(pixels[offset]) + 0.7152 * Double(pixels[offset + 1]) + 0.0722 * Double(pixels[offset + 2])) / 255
        weightedLightness += lightness * alpha
        alphaSum += alpha
    }
    precondition(alphaSum > 0, "Empty Rex artwork: \(name)")
    let tile = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 48, pixelsHigh: 48, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: tile)
    NSColor(calibratedWhite: weightedLightness / alphaSum > 0.65 ? 0.18 : 0.97, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: 48, height: 48), xRadius: 7, yRadius: 7).fill()
    image.draw(in: NSRect(x: 4, y: 4, width: 40, height: 40), from: .zero, operation: .sourceOver, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    let png = tile.representation(using: .png, properties: [:])!
    precondition(png.starts(with: [137, 80, 78, 71, 13, 10, 26, 10]))
    try png.write(to: destination.appendingPathComponent(String(name.dropFirst("block-glyph-".count)) + ".png"))
}
print("Exported \(names.count) Rex program icons at 48×48")
