// Draws assets/AppIcon.icns: a serif "N" with a gold period on deep blue.
// Usage: swift scripts/make-icon.swift
import AppKit

func render(_ px: Int) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4,
                               hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let s = CGFloat(px) / 1024
    // macOS icon grid: 824 pt artwork centred in 1024
    let art = NSRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
    let body = NSBezierPath(roundedRect: art, xRadius: 185 * s, yRadius: 185 * s)
    NSGradient(starting: NSColor(red: 0.20, green: 0.33, blue: 0.56, alpha: 1), ending: NSColor(red: 0.10, green: 0.19, blue: 0.36, alpha: 1))!
        .draw(in: body, angle: -90)

    let font = NSFont(name: "IowanOldStyle-Bold", size: 560 * s) ?? NSFont.boldSystemFont(ofSize: 560 * s)
    let paper = NSColor(red: 0.96, green: 0.95, blue: 0.92, alpha: 1)
    let letter = NSAttributedString(string: "N", attributes: [.font: font, .foregroundColor: paper])
    let size = letter.size()
    let dot = 74 * s
    let gap = 18 * s
    let total = size.width + gap + dot
    let x = art.midX - total / 2
    let y = art.midY - size.height / 2 + 6 * s
    letter.draw(at: NSPoint(x: x, y: y))
    NSColor(red: 0.79, green: 0.73, blue: 0.47, alpha: 1).setFill()
    // sit the period on the baseline of the letter
    NSBezierPath(ovalIn: NSRect(x: x + size.width + gap, y: y - font.descender - dot * 0.15, width: dot, height: dot)).fill()
    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let iconset = "assets/AppIcon.iconset"
try? FileManager.default.removeItem(atPath: iconset)
try! FileManager.default.createDirectory(atPath: iconset, withIntermediateDirectories: true)
for (name, px) in [("16x16", 16), ("16x16@2x", 32), ("32x32", 32), ("32x32@2x", 64), ("128x128", 128), ("128x128@2x", 256),
                   ("256x256", 256), ("256x256@2x", 512), ("512x512", 512), ("512x512@2x", 1024)] {
    let data = render(px).representation(using: .png, properties: [:])!
    try! data.write(to: URL(fileURLWithPath: "\(iconset)/icon_\(name).png"))
}
print("iconset written")
