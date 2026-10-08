// Draws the app icon (ink-blue tile, a plain white envelope) and writes
// crates/jm-app/assets/JustMail.icns. Run: swift tools/make-icon.swift
import AppKit

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()

func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(red: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
}

func draw(size: Int) -> Data {
    let s = CGFloat(size) / 1024
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                        space: CGColorSpace(name: CGColorSpace.sRGB)!,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.scaleBy(x: s, y: s)

    // macOS icon grid: 824 pt tile with a soft drop shadow
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: rgb(0x000000, 0.35))
    ctx.addPath(shape)
    ctx.setFillColor(rgb(0x1f3a8a))
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    let gradient = CGGradient(colorsSpace: nil, colors: [rgb(0x3b6fe0), rgb(0x1b2f7a)] as CFArray,
                              locations: [0, 1])!
    ctx.drawLinearGradient(gradient, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
    ctx.restoreGState()

    // the envelope
    let env = CGRect(x: 232, y: 300, width: 560, height: 400)
    let body = CGPath(roundedRect: env, cornerWidth: 44, cornerHeight: 44, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 26, color: rgb(0x000000, 0.30))
    ctx.addPath(body)
    ctx.setFillColor(rgb(0xffffff))
    ctx.fillPath()
    ctx.restoreGState()

    // the flap: a V from the top corners down to the middle
    ctx.saveGState()
    ctx.addPath(body)
    ctx.clip()
    ctx.setStrokeColor(rgb(0x1f3a8a, 0.9))
    ctx.setLineWidth(30)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.move(to: CGPoint(x: env.minX + 40, y: env.maxY - 40))
    ctx.addLine(to: CGPoint(x: env.midX, y: env.midY - 10))
    ctx.addLine(to: CGPoint(x: env.maxX - 40, y: env.maxY - 40))
    ctx.strokePath()
    ctx.restoreGState()

    let image = ctx.makeImage()!
    let rep = NSBitmapImageRep(cgImage: image)
    return rep.representation(using: .png, properties: [:])!
}

let iconset = FileManager.default.temporaryDirectory.appendingPathComponent("JustMail.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
for (name, px) in [("16x16", 16), ("16x16@2x", 32), ("32x32", 32), ("32x32@2x", 64), ("128x128", 128),
                   ("128x128@2x", 256), ("256x256", 256), ("256x256@2x", 512), ("512x512", 512),
                   ("512x512@2x", 1024)] {
    try! draw(size: px).write(to: iconset.appendingPathComponent("icon_\(name).png"))
}
let assets = root.appendingPathComponent("crates/jm-app/assets")
try! FileManager.default.createDirectory(at: assets, withIntermediateDirectories: true)
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconset.path, "-o", assets.appendingPathComponent("JustMail.icns").path]
try! task.run()
task.waitUntilExit()
try! draw(size: 512).write(to: assets.appendingPathComponent("icon.png"))
print("wrote \(assets.path)/JustMail.icns")
