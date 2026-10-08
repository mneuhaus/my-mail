// Prints the CGWindowID of the first on-screen window of a process, for `screencapture -l`.
// Usage: swift tools/winid.swift [process-name] [pid]
import CoreGraphics
let args = CommandLine.arguments
let name = args.count > 1 ? args[1] : "just-mail"
let pid = args.count > 2 ? Int(args[2]) : nil
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerName as String] as? String) == name && (w[kCGWindowLayer as String] as? Int) == 0 {
    if let pid = pid, (w[kCGWindowOwnerPID as String] as? Int) != pid { continue }
    print(w[kCGWindowNumber as String] as! Int)
    break
}
