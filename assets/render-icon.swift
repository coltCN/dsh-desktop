#!/usr/bin/env swift
// Render the dsh desktop icon to a 1024x1024 TRANSPARENT PNG using
// CoreGraphics + CoreText (qlmanage filled the canvas white — useless for
// app icons). Coordinates mirror assets/dsh-desktop-icon.svg (top-down SVG
// semantics converted to CoreGraphics bottom-up coordinates).

import AppKit
import CoreText
import Foundation

let W: CGFloat = 1024
let H: CGFloat = 1024

guard let ctx = CGContext(
    data: nil, width: Int(W), height: Int(H),
    bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpaceCreateDeviceRGB(),
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else { fatalError("no context") }

// 1. Rounded-rect background gradient (SVG rect x=64 y=64 w=896 h=896 rx=208)
let bgRect = CGRect(x: 64, y: 64, width: 896, height: 896)
let bgPath = CGPath(roundedRect: bgRect, cornerWidth: 208, cornerHeight: 208, transform: nil)
ctx.saveGState()
ctx.addPath(bgPath)
ctx.clip()
let colors = [
    CGColor(red: 0x23 / 255.0, green: 0x2a / 255.0, blue: 0x38 / 255.0, alpha: 1), // #232a38
    CGColor(red: 0x14 / 255.0, green: 0x17 / 255.0, blue: 0x1f / 255.0, alpha: 1), // #14171f
] as CFArray
let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: [0, 1])!
ctx.drawLinearGradient(grad, start: CGPoint(x: 64, y: 960), end: CGPoint(x: 960, y: 64), options: [])
ctx.restoreGState()

// 2. Outline stroke (SVG stroke #6c8cff @55% width 14)
ctx.saveGState()
ctx.addPath(bgPath)
ctx.setStrokeColor(CGColor(red: 0x6c / 255.0, green: 0x8c / 255.0, blue: 1.0, alpha: 0.55))
ctx.setLineWidth(14)
ctx.strokePath()
ctx.restoreGState()

// 3. Text helpers. CoreText positions the baseline; CoreGraphics origin is
// bottom-left, so an SVG baseline at top-down y becomes CG y = H - svgY.
func ctFont(_ name: String, _ size: CGFloat, bold: Bool) -> CTFont {
    let base = CTFontCreateWithName(name as CFString, size, nil)
    guard bold, let boldFont = CTFontCreateCopyWithSymbolicTraits(
        base, size, nil, CTFontSymbolicTraits.boldTrait, CTFontSymbolicTraits.boldTrait)
    else { return base }
    return boldFont
}

func textWidth(_ font: CTFont, _ text: String) -> CGFloat {
    let attr = NSAttributedString(string: text, attributes: [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
    ])
    let line = CTLineCreateWithAttributedString(attr)
    var ascent: CGFloat = 0
    var descent: CGFloat = 0
    var leading: CGFloat = 0
    let width = CGFloat(CTLineGetTypographicBounds(line, &ascent, &descent, &leading))
    return width
}

func drawText(_ text: String, font: CTFont, color: CGColor, centerX: CGFloat? = nil, x: CGFloat? = nil, baselineTopDownY svgY: CGFloat) {
    let attr = NSAttributedString(string: text, attributes: [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): color,
    ])
    let line = CTLineCreateWithAttributedString(attr)
    let width = textWidth(font, text)
    let px: CGFloat
    if let cx = centerX {
        px = cx - width / 2
    } else if let x = x {
        px = x
    } else {
        px = (W - width) / 2
    }
    ctx.textPosition = CGPoint(x: px, y: H - svgY)
    CTLineDraw(line, ctx)
}

let mono = "SF Mono" // falls back through CT if absent
let textColor = CGColor(red: 0xe8 / 255.0, green: 0xea / 255.0, blue: 0xed / 255.0, alpha: 1) // #e8eaed
let accent = CGColor(red: 0x6c / 255.0, green: 0x8c / 255.0, blue: 1.0, alpha: 1) // #6c8cff

// "dsh" — SVG: font 236 weight 600, text-anchor middle at x=512, baseline y=520
let dshFont = ctFont("SFMono-Bold", 236, bold: true)
drawText("dsh", font: dshFont, color: textColor, centerX: 512, baselineTopDownY: 520)

// "$" — SVG: font 150, anchor middle at x=600, baseline y=742
let dollarFont = ctFont("SFMono-Regular", 150, bold: false)
drawText("$", font: dollarFont, color: accent, centerX: 600, baselineTopDownY: 742)

// cursor block — SVG: rect x=692 y=600 w=34 h=118 fill #6c8cff
ctx.setFillColor(accent)
ctx.fill(CGRect(x: 692, y: H - 718, width: 34, height: 118))

// 4. Export PNG
guard let image = ctx.makeImage() else { fatalError("no image") }
let rep = NSBitmapImageRep(cgImage: image)
guard let png = rep.representation(using: .png, properties: [:]) else { fatalError("no png") }
let out = URL(fileURLWithPath: "/tmp/dsh-desktop-icon-transparent.png")
try! png.write(to: out)
print("wrote \(out.path) \(image.width)x\(image.height)")
