// AC-4b fault: the S-01 state in which macOS does not rejoin on its own.
import CoreWLAN
import Foundation
guard let iface = CWWiFiClient.shared().interface() else { print("no Wi-Fi interface"); exit(1) }
let f = DateFormatter(); f.dateFormat = "HH:mm:ss"
// Taken before the call: a drop time rounded early only makes the measured delays longer.
let at = f.string(from: Date())
iface.disassociate()
print("dropped \(at)")
