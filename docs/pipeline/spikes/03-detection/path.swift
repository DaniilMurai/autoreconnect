import Network
import Foundation
let m = NWPathMonitor()
m.pathUpdateHandler = { p in print(Date(), p.status, p.isExpensive, p.usesInterfaceType(.wifi), p.supportsDNS, p.supportsIPv4) }
m.start(queue: .global())
Thread.sleep(forTimeInterval: 3)
