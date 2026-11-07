import AppKit
let destination = CommandLine.arguments[1]
try FileManager.default.createDirectory(atPath: destination, withIntermediateDirectories: true)
for (points, scale) in [(16,1),(16,2),(32,1),(32,2),(128,1),(128,2),(256,1),(256,2),(512,1),(512,2)] {
    let side = points * scale
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    let size = CGFloat(side)
    NSColor(calibratedRed: 0.15, green: 0.33, blue: 0.27, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: size*0.06, y: size*0.06, width: size*0.88, height: size*0.88), xRadius: size*0.2, yRadius: size*0.2).fill()
    let center = NSPoint(x: size/2, y: size/2)
    for radius in [0.30,0.24,0.18] {
        let circle = NSBezierPath(ovalIn: NSRect(x: center.x-size*radius, y: center.y-size*radius, width: size*radius*2, height: size*radius*2))
        NSColor(calibratedRed: 0.77, green: 0.89, blue: 0.80, alpha: radius == 0.30 ? 0.9 : 0.45).setStroke()
        circle.lineWidth = size*0.022
        circle.stroke()
    }
    NSColor(calibratedRed: 0.95, green: 0.96, blue: 0.87, alpha: 1).setFill()
    let play = NSBezierPath(); play.move(to: NSPoint(x: size*0.46,y:size*0.40));play.line(to:NSPoint(x:size*0.46,y:size*0.60));play.line(to:NSPoint(x:size*0.62,y:size*0.50));play.close();play.fill()
    NSGraphicsContext.restoreGraphicsState()
    let name = "icon_\(points)x\(points)" + (scale == 2 ? "@2x" : "") + ".png"
    try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: destination).appendingPathComponent(name))
}
