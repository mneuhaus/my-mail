// Prints the CGWindowID of the first on-screen window owned by the given process name.
import CoreGraphics
let name = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "just-mail"
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerName as String] as? String) == name && (w[kCGWindowLayer as String] as? Int) == 0 {
    print(w[kCGWindowNumber as String] as! Int)
    break
}
