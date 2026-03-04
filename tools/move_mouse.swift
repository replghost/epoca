import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 3,
      let x = Double(CommandLine.arguments[1]),
      let y = Double(CommandLine.arguments[2]) else {
    fputs("Usage: move_mouse <x> <y>\n", stderr)
    exit(1)
}

let point = CGPoint(x: x, y: y)
if let event = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left) {
    event.post(tap: .cghidEventTap)
}
