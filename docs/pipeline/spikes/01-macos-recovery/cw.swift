import CoreWLAN
import Foundation
let i = CWWiFiClient.shared().interface()!
switch CommandLine.arguments.dropFirst().first ?? "info" {
case "disassociate":
    i.disassociate()
    print("disassociate called")
default:
    print("name=\(i.interfaceName ?? "nil") power=\(i.powerOn()) ssid=\(i.ssid() == nil ? "nil" : "present") bssid=\(i.bssid() == nil ? "nil" : "present") rssi=\(i.rssiValue()) mode=\(i.interfaceMode().rawValue)")
}
