import Cocoa
import CoreWLAN
import CoreLocation

let t0 = Date()
func log(_ s: String) { print(String(format: "[%6.2f] ", Date().timeIntervalSince(t0)) + s); fflush(stdout) }

func online() -> Bool {
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/bin/curl")
    p.arguments = ["--interface", "en0", "-s", "-m", "2", "-o", "/dev/null", "-w", "%{http_code}", "http://captive.apple.com/hotspot-detect.html"]
    let pipe = Pipe(); p.standardOutput = pipe
    try? p.run(); p.waitUntilExit()
    return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) == "200"
}
func waitOnline(_ secs: Int) -> Bool {
    let start = Date()
    while Date().timeIntervalSince(start) < Double(secs) { if online() { return true }; Thread.sleep(forTimeInterval: 0.5) }
    return false
}

let iface = CWWiFiClient.shared().interface()!
let phase = CommandLine.arguments.dropFirst().first ?? "a"

func keychainPassword(_ ssid: Data) -> String? {
    for (name, dom) in (phase == "a" ? [("user", CWKeychainDomain.user), ("system", CWKeychainDomain.system)] : [("user", CWKeychainDomain.user)]) {
        var pw: NSString?
        let s = Date()
        let rc = CWKeychainFindWiFiPassword(dom, ssid, &pw)
        log("keychain \(name): rc=\(rc) found=\(pw != nil) took=\(String(format: "%.1f", Date().timeIntervalSince(s)))s")
        if let pw = pw { return pw as String }
    }
    return nil
}

// Phase r: rejoin from an already disassociated state; the SSID comes from the preferred list.
func rejoin() {
    let profiles = iface.configuration()?.networkProfiles.array as? [CWNetworkProfile] ?? []
    log("phase=r mode=\(iface.interfaceMode().rawValue) online(en0)=\(online()) profiles=\(profiles.count)")
    guard let first = profiles.first, let ssid = first.ssid else { log("no preferred network"); exit(2) }
    var s = Date()
    let nets = (try? iface.scanForNetworks(withName: ssid)) ?? []
    log("scan for preferred #0: \(nets.count) BSS, took \(String(format: "%.1f", Date().timeIntervalSince(s)))s")
    guard let net = nets.max(by: { $0.rssiValue < $1.rssiValue }) else { log("not in range"); exit(3) }
    s = Date()
    do { try iface.associate(to: net, password: nil); log("associate(nil) ok in \(String(format: "%.1f", Date().timeIntervalSince(s)))s") }
    catch { log("associate(nil) error after \(String(format: "%.1f", Date().timeIntervalSince(s)))s: \(error)") }
    for _ in 0..<40 {
        if iface.interfaceMode().rawValue != 0 && online() { log("ASSOCIATED+ONLINE(en0) \(String(format: "%.1f", Date().timeIntervalSince(s)))s after associate"); exit(0) }
        Thread.sleep(forTimeInterval: 0.5)
    }
    log("still not associated/online on en0: mode=\(iface.interfaceMode().rawValue)"); exit(1)
}

// Phase k: password from the System keychain (user approves the prompt), then rejoin with it.
func keychainRejoin() {
    let profiles = iface.configuration()?.networkProfiles.array as? [CWNetworkProfile] ?? []
    guard let ssidData = profiles.first?.ssidData, let ssid = profiles.first?.ssid else { log("no preferred network"); exit(2) }
    log("phase=k mode=\(iface.interfaceMode().rawValue) online(en0)=\(online())")
    var pw: String?
    for attempt in 1...2 {
        var out: NSString?
        let s = Date()
        let rc = CWKeychainFindWiFiPassword(.system, ssidData, &out)
        log("system keychain read #\(attempt): rc=\(rc) found=\(out != nil) took=\(String(format: "%.1f", Date().timeIntervalSince(s)))s")
        if let out = out { pw = out as String }
    }
    guard let pw = pw else { log("no password -> stop"); exit(5) }
    if iface.interfaceMode().rawValue != 0 { iface.disassociate(); log("was associated -> disassociated"); Thread.sleep(forTimeInterval: 1) }
    var s = Date()
    let nets = (try? iface.scanForNetworks(withName: ssid)) ?? []
    log("scan: \(nets.count) BSS, took \(String(format: "%.1f", Date().timeIntervalSince(s)))s")
    guard let net = nets.max(by: { $0.rssiValue < $1.rssiValue }) else { log("not in range"); exit(3) }
    s = Date()
    do { try iface.associate(to: net, password: pw); log("associate(pw) ok in \(String(format: "%.1f", Date().timeIntervalSince(s)))s") }
    catch { log("associate(pw) error after \(String(format: "%.1f", Date().timeIntervalSince(s)))s: \(error)") }
    for _ in 0..<40 {
        if iface.interfaceMode().rawValue != 0 && online() { log("ASSOCIATED+ONLINE(en0) \(String(format: "%.1f", Date().timeIntervalSince(s)))s after associate"); exit(0) }
        Thread.sleep(forTimeInterval: 0.5)
    }
    log("still not associated/online on en0: mode=\(iface.interfaceMode().rawValue)"); exit(1)
}

// Phase p: keychain reads only, no network changes; shows whether "Always Allow" makes later reads silent.
func keychainOnly() {
    let profiles = iface.configuration()?.networkProfiles.array as? [CWNetworkProfile] ?? []
    guard let ssidData = profiles.first?.ssidData else { log("no preferred network"); exit(2) }
    for attempt in 1...3 {
        var out: NSString?
        let s = Date()
        let rc = CWKeychainFindWiFiPassword(.system, ssidData, &out)
        log("system keychain read #\(attempt): rc=\(rc) found=\(out != nil) took=\(String(format: "%.2f", Date().timeIntervalSince(s)))s")
    }
    exit(0)
}

// Phase s (spike S-07): measure the OS's own rejoin after the user switches the AP off/on.
// The system's cached scan results are not refreshed while disconnected (run0 showed
// "visible" only at association), so we scan ourselves; resolution = one scan (~2-5 s).
func baseline() {
    let cycles = 5
    guard let ssid = iface.ssid(), online() else { log("start associated and online on en0 first"); exit(2) }
    log("baseline: target network captured, \(cycles) cycles; switch the AP off for ~60 s, then on")
    var times: [Double] = []
    for c in 1...cycles {
        // Only a real Wi-Fi drop counts; an internet-only outage (Wi-Fi still associated) is skipped.
        while iface.interfaceMode().rawValue != 0 { Thread.sleep(forTimeInterval: 0.5) }
        let tDown = Date(); log("cycle \(c): DOWN (Wi-Fi disassociated)")
        var tVisible: Date?
        var lastMiss = tDown
        var bound = ""
        var tAssoc: Date?
        while true {
            let assoc = iface.interfaceMode().rawValue != 0 && iface.ssid() == ssid
            if tVisible == nil && assoc {
                tVisible = lastMiss; bound = " (upper bound: associated before our scan saw the AP)"
                log("cycle \(c): ASSOCIATED before scan saw it")
            }
            if tVisible == nil {
                let t1 = Date()
                let seen = ((try? iface.scanForNetworks(withName: ssid)) ?? []).count > 0
                if seen { tVisible = Date(); log("cycle \(c): VISIBLE (scan took \(String(format: "%.1f", Date().timeIntervalSince(t1)))s)") }
                else { lastMiss = Date() }
                continue
            }
            if assoc && tAssoc == nil { tAssoc = Date(); log("cycle \(c): ASSOCIATED +\(String(format: "%.1f", tAssoc!.timeIntervalSince(tVisible!)))s after visible") }
            if assoc && online() {
                let dt = Date().timeIntervalSince(tVisible!); times.append(dt)
                log("cycle \(c): ONLINE  time_back_online=\(String(format: "%.1f", dt))s\(bound) (down for \(String(format: "%.0f", Date().timeIntervalSince(tDown)))s)")
                break
            }
            if Date().timeIntervalSince(tVisible!) > 180 {
                log("cycle \(c): OS did NOT rejoin within 180 s of the network being visible -> rejoin by hand")
                while !online() { Thread.sleep(forTimeInterval: 1) }
                break
            }
            Thread.sleep(forTimeInterval: 0.25)
        }
        Thread.sleep(forTimeInterval: 5)
    }
    let sorted = times.sorted()
    log("RESULT n=\(times.count) times=\(times.map { String(format: "%.1f", $0) }) median=\(sorted.isEmpty ? "n/a" : String(format: "%.1f", sorted[sorted.count / 2]))s")
    exit(0)
}

func run() {
    if phase == "s" { baseline() }
    if phase == "p" { keychainOnly() }
    if phase == "r" { rejoin() }
    if phase == "k" { keychainRejoin() }
    log("phase=\(phase) auth=\(CLLocationManager().authorizationStatus.rawValue) mode=\(iface.interfaceMode().rawValue)")
    guard let ssid = iface.ssid(), let ssidData = iface.ssidData() else { log("ssid=nil -> abort"); exit(2) }
    let profiles = iface.configuration()?.networkProfiles.array as? [CWNetworkProfile] ?? []
    log("ssid readable, len=\(ssid.count); index in preferred list=\(profiles.firstIndex { $0.ssid == ssid }.map(String.init) ?? "none") of \(profiles.count)")
    var s = Date()
    let nets = (try? iface.scanForNetworks(withName: ssid)) ?? []
    log("scan: \(nets.count) BSS for current ssid, took \(String(format: "%.1f", Date().timeIntervalSince(s)))s")
    guard let net = nets.max(by: { $0.rssiValue < $1.rssiValue }) else { log("no network in scan -> abort"); exit(3) }
    log("best rssi=\(net.rssiValue) wpa2psk=\(net.supportsSecurity(.wpa2Personal)) wpa3=\(net.supportsSecurity(.wpa3Personal))")
    let pw = keychainPassword(ssidData)
    if phase == "a" { log("phase a done"); exit(0) }

    // Phase b: drop, then rejoin. Hard cap on offline time.
    iface.disassociate(); let tDrop = Date()
    log("disassociated, mode=\(iface.interfaceMode().rawValue)")
    if phase == "c" {
        if waitOnline(15) { log("CONTROL: macOS auto-joined, ONLINE \(String(format: "%.1f", Date().timeIntervalSince(tDrop)))s after drop, no associate call"); exit(0) }
        log("CONTROL: still offline after 15 s without associate -> trying associate(nil)")
    } else { Thread.sleep(forTimeInterval: 2) }
    s = Date()
    do { try iface.associate(to: net, password: nil); log("associate(nil) ok in \(String(format: "%.1f", Date().timeIntervalSince(s)))s") }
    catch { log("associate(nil) error: \(error)") }
    if waitOnline(15) { log("ONLINE \(String(format: "%.1f", Date().timeIntervalSince(tDrop)))s after drop (via nil password)"); exit(0) }
    if let pw = pw {
        s = Date()
        do { try iface.associate(to: net, password: pw); log("associate(pw) ok in \(String(format: "%.1f", Date().timeIntervalSince(s)))s") }
        catch { log("associate(pw) error: \(error)") }
        if waitOnline(15) { log("ONLINE \(String(format: "%.1f", Date().timeIntervalSince(tDrop)))s after drop (via keychain password)"); exit(0) }
    }
    log("FAILED to get online \(String(format: "%.1f", Date().timeIntervalSince(tDrop)))s after drop -> stop, user must rejoin")
    exit(1)
}

class D: NSObject, CLLocationManagerDelegate {
    let m = CLLocationManager()
    func locationManagerDidChangeAuthorization(_ m: CLLocationManager) {
        log("auth changed: \(m.authorizationStatus.rawValue)")
        if m.authorizationStatus != .notDetermined { DispatchQueue.global().async { run() } }
    }
}
if phase == "p" { DispatchQueue.global().async { run() } }
let d = D(); if phase != "p" { d.m.delegate = d }
if phase != "p" && d.m.authorizationStatus == .notDetermined { log("requesting location authorization"); d.m.requestWhenInUseAuthorization() }
if phase != "s" { DispatchQueue.main.asyncAfter(deadline: .now() + 240) { log("timeout waiting for authorization"); exit(4) } }
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
