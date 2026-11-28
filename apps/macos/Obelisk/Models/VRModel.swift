import AppKit
import Darwin
import Foundation
import Observation
import os

/// Quality preset for VR video streaming to a Meta Quest headset.
enum VRQualityPreset: String, CaseIterable, Identifiable, Sendable {
    case low, medium, high, ultra, custom

    var id: String { rawValue }

    var title: String {
        switch self {
        case .low: L10n.string("ui.vr.preset.low")
        case .medium: L10n.string("ui.vr.preset.medium")
        case .high: L10n.string("ui.vr.preset.high")
        case .ultra: L10n.string("ui.vr.preset.ultra")
        case .custom: L10n.string("ui.vr.preset.custom")
        }
    }

    var parameters: (eye: Int, fps: Int, mbps: Int, foveation: VRFoveation)? {
        switch self {
        case .low: (1024, 72, 60, .off)
        case .medium: (1440, 90, 100, .off)
        case .high: (1832, 90, 150, .off)
        case .ultra: (2048, 90, 200, .off)
        case .custom: nil
        }
    }
}

enum VRCodec: String, CaseIterable, Identifiable, Sendable {
    case hevc, h264

    var id: String { rawValue }

    var title: String {
        switch self {
        case .hevc: L10n.string("ui.vr.codec.hevc")
        case .h264: L10n.string("ui.vr.codec.h264")
        }
    }
}

enum VRAudioRouting: String, CaseIterable, Identifiable, Sendable {
    case quest, mac, both

    var id: String { rawValue }

    var title: String {
        switch self {
        case .quest: L10n.string("ui.vr.audio.quest")
        case .mac: L10n.string("ui.vr.audio.mac")
        case .both: L10n.string("ui.vr.audio.both")
        }
    }
}

enum VRFoveation: String, CaseIterable, Identifiable, Sendable {
    case off, light, medium, strong

    var id: String { rawValue }

    var title: String {
        switch self {
        case .off: L10n.string("ui.vr.foveation.off")
        case .light: L10n.string("ui.vr.foveation.light")
        case .medium: L10n.string("ui.vr.foveation.medium")
        case .strong: L10n.string("ui.vr.foveation.strong")
        }
    }

    var level: Int {
        switch self {
        case .off: 0
        case .light: 1
        case .medium: 2
        case .strong: 3
        }
    }

    func envValue(centerSuffix: String?) -> String {
        guard level > 0 else { return "0" }
        let suffix = (centerSuffix?.isEmpty == false) ? centerSuffix! : "1,0.5,0.5,0.5"
        return "\(level),\(suffix)"
    }

    static func centerSuffix(from envValue: String) -> String? {
        let parts = envValue.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count >= 5 else { return nil }
        let suffix = parts.dropFirst().prefix(4).joined(separator: ",")
        // Ignore placeholder centered values so we never overwrite a real headset lens center.
        guard suffix != "1,0.5,0.5,0.5" && suffix != "1,0.5000,0.5000,0.5000" else { return nil }
        return suffix
    }

    static func from(envValue: String) -> VRFoveation {
        let level = envValue.split(separator: ",").first.flatMap { Int($0) } ?? 0
        switch level {
        case 1: return .light
        case 2: return .medium
        case 3...: return .strong
        default: return .off
        }
    }
}

enum VRFrameGenMode: Int, CaseIterable, Identifiable, Sendable {
    case off = 0
    case autoFill = 1
    case spaceWarp = 2

    var id: Int { rawValue }

    var title: String {
        switch self {
        case .off: L10n.string("ui.vr.framegen.off")
        case .autoFill: L10n.string("ui.vr.framegen.auto_fill")
        case .spaceWarp: L10n.string("ui.vr.framegen.spacewarp")
        }
    }
}

/// An installed Windows game discovered across Obelisk's storefronts for VR play.
struct VRGameItem: Identifiable, Sendable {
    let id: String
    let title: String
    let storefront: LibrarySection
    let directory: String
    let detection: VrGameDetection

    var folderName: String {
        URL(fileURLWithPath: directory).lastPathComponent
    }
}

/// Background UDP sockets that keep Obelisk FX discoverable to the VRmin Quest APK
/// over Wi-Fi (UDP 9101) and USB-C (IPv6 UDP 9103), and receive local control
/// commands from ObeliskVRRenderer on UDP 9102.
private final class VRNetworkBridge: @unchecked Sendable {
    struct Handlers: Sendable {
        let onWakeRequest: @Sendable () -> Void
        let onSettingsUpdate: @Sendable (Int?, Int?, Int?, String?, String?, String?) -> Void
        let onGameConfigUpdate: @Sendable (Int?) -> Void
        let onPlayGameWithQuality: @Sendable (String?, Int, Int, Int, String) -> Void
        let onLaunchGame: @Sendable (String) -> Void
        let onQuitGame: @Sendable () -> Void
    }

    private let runningState = OSAllocatedUnfairLock(initialState: false)
    private let stoppedState = OSAllocatedUnfairLock(initialState: false)
    private let macName: String
    private let handlers: Handlers

    init(macName: String, handlers: Handlers) {
        self.macName = macName
        self.handlers = handlers
    }

    func setServerRunning(_ running: Bool) {
        runningState.withLock { $0 = running }
    }

    func stop() {
        stoppedState.withLock { $0 = true }
    }

    func start() {
        startWifiBeacon()
        startUsbProbeResponder()
        startControlListener()
    }

    private var isStopped: Bool { stoppedState.withLock { $0 } }
    private var isServerRunning: Bool { runningState.withLock { $0 } }

    private func startWifiBeacon() {
        let name = macName
        Thread.detachNewThread { [weak self] in
            let sock = socket(AF_INET, SOCK_DGRAM, 0)
            guard sock >= 0 else { return }
            defer { close(sock) }
            var one: Int32 = 1
            setsockopt(sock, SOL_SOCKET, SO_BROADCAST, &one, socklen_t(MemoryLayout<Int32>.size))
            let msg = Array("{\"qvb\":1,\"port\":9100,\"idle\":true,\"name\":\"\(name)\"}".utf8)

            while let self, !self.isStopped {
                if !self.isServerRunning {
                    var targets: [in_addr_t] = [in_addr_t(0xffff_ffff)]
                    var ifap: UnsafeMutablePointer<ifaddrs>?
                    if getifaddrs(&ifap) == 0 {
                        var cursor = ifap
                        while let current = cursor {
                            let iface = current.pointee
                            if let addr = iface.ifa_addr, addr.pointee.sa_family == sa_family_t(AF_INET),
                               (iface.ifa_flags & UInt32(IFF_BROADCAST)) != 0,
                               (iface.ifa_flags & UInt32(IFF_UP)) != 0,
                               (iface.ifa_flags & UInt32(IFF_LOOPBACK)) == 0,
                               let dst = iface.ifa_dstaddr {
                                dst.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
                                    targets.append($0.pointee.sin_addr.s_addr)
                                }
                            }
                            cursor = iface.ifa_next
                        }
                        freeifaddrs(ifap)
                    }
                    var addr = sockaddr_in()
                    addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
                    addr.sin_family = sa_family_t(AF_INET)
                    addr.sin_port = UInt16(9101).bigEndian
                    for target in targets {
                        addr.sin_addr.s_addr = target
                        _ = withUnsafePointer(to: &addr) {
                            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                                sendto(sock, msg, msg.count, 0, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                            }
                        }
                    }
                }
                sleep(1)
            }
        }
    }

    private func startUsbProbeResponder() {
        let name = macName
        Thread.detachNewThread { [weak self] in
            let sock = socket(AF_INET6, SOCK_DGRAM, 0)
            guard sock >= 0 else { return }
            defer { close(sock) }
            var one: Int32 = 1
            setsockopt(sock, SOL_SOCKET, SO_REUSEADDR, &one, socklen_t(MemoryLayout<Int32>.size))
            var timeout = timeval(tv_sec: 1, tv_usec: 0)
            setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))

            var addr = sockaddr_in6()
            addr.sin6_len = UInt8(MemoryLayout<sockaddr_in6>.size)
            addr.sin6_family = sa_family_t(AF_INET6)
            addr.sin6_port = UInt16(9103).bigEndian
            addr.sin6_addr = in6addr_any
            let bound = withUnsafePointer(to: &addr) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    bind(sock, $0, socklen_t(MemoryLayout<sockaddr_in6>.size))
                }
            }
            guard bound == 0 else { return }

            var buf = [UInt8](repeating: 0, count: 256)
            while let self, !self.isStopped {
                var from = sockaddr_in6()
                var fl = socklen_t(MemoryLayout<sockaddr_in6>.size)
                let n = withUnsafeMutablePointer(to: &from) {
                    $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                        recvfrom(sock, &buf, buf.count, 0, $0, &fl)
                    }
                }
                guard n > 0, String(decoding: buf.prefix(n), as: UTF8.self).contains("probe") else { continue }
                let idle = self.isServerRunning ? "false" : "true"
                let reply = Array("{\"qvb\":1,\"port\":9100,\"idle\":\(idle),\"usb\":true,\"name\":\"\(name)\"}".utf8)
                _ = withUnsafePointer(to: &from) {
                    $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                        sendto(sock, reply, reply.count, 0, $0, fl)
                    }
                }
            }
        }
    }

    private func startControlListener() {
        Thread.detachNewThread { [weak self] in
            let sock = socket(AF_INET6, SOCK_DGRAM, 0)
            guard sock >= 0 else { return }
            defer { close(sock) }
            var one: Int32 = 1
            var zero: Int32 = 0
            setsockopt(sock, SOL_SOCKET, SO_REUSEADDR, &one, socklen_t(MemoryLayout<Int32>.size))
            setsockopt(sock, Int32(IPPROTO_IPV6), IPV6_V6ONLY, &zero, socklen_t(MemoryLayout<Int32>.size))
            var timeout = timeval(tv_sec: 1, tv_usec: 0)
            setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))

            var addr = sockaddr_in6()
            addr.sin6_len = UInt8(MemoryLayout<sockaddr_in6>.size)
            addr.sin6_family = sa_family_t(AF_INET6)
            addr.sin6_port = UInt16(9102).bigEndian
            addr.sin6_addr = in6addr_any
            let bound = withUnsafePointer(to: &addr) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    bind(sock, $0, socklen_t(MemoryLayout<sockaddr_in6>.size))
                }
            }
            guard bound == 0 else { return }

            var buf = [UInt8](repeating: 0, count: 1024)
            while let self, !self.isStopped {
                var from = sockaddr_in6()
                var fl = socklen_t(MemoryLayout<sockaddr_in6>.size)
                let n = withUnsafeMutablePointer(to: &from) {
                    $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                        recvfrom(sock, &buf, buf.count, 0, $0, &fl)
                    }
                }
                guard n > 0 else { continue }
                let text = String(decoding: buf.prefix(n), as: UTF8.self)
                let addrBytes = withUnsafeBytes(of: from.sin6_addr) { Array($0) }
                let isLocal = addrBytes == [UInt8](repeating: 0, count: 15) + [1]
                    || (addrBytes.prefix(10).allSatisfy { $0 == 0 } && addrBytes[10] == 0xff && addrBytes[11] == 0xff && addrBytes[12] == 127)

                if isLocal, let obj = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
                   let kind = obj["qvb"] as? String {
                    switch kind {
                    case "settings":
                        self.handlers.onSettingsUpdate(
                            obj["eye"] as? Int,
                            obj["fps"] as? Int,
                            obj["mbps"] as? Int,
                            obj["codec"] as? String,
                            obj["audio"] as? String,
                            obj["fov"] as? String
                        )
                    case "gamecfg":
                        self.handlers.onGameConfigUpdate(obj["scale"] as? Int)
                    case "playgame":
                        if let eye = obj["eye"] as? Int, let fps = obj["fps"] as? Int,
                           let mbps = obj["mbps"] as? Int, let fov = obj["fov"] as? String {
                            self.handlers.onPlayGameWithQuality(obj["id"] as? String, eye, fps, mbps, fov)
                        }
                    case "obelisk_launch":
                        if let id = obj["id"] as? String {
                            self.handlers.onLaunchGame(id)
                        }
                    case "obelisk_quit":
                        self.handlers.onQuitGame()
                    default:
                        break
                    }
                    continue
                }
                if text.contains("\"start\"") {
                    self.handlers.onWakeRequest()
                }
            }
        }
    }
}

/// Manages Obelisk's bundled Metal VR streaming renderer (adapted from VRmin by thevermin),
/// Quest headset pairing/discovery, streaming quality settings, and VR game scanning.
@MainActor
@Observable
final class VRModel {
    static let vrminRepository = URL(string: "https://github.com/thevermin/vrmin")!
    static let eyeOptions = [1024, 1280, 1440, 1536, 1832, 2048, 2304, 2560]
    static let fpsOptions = [72, 80, 90, 120]

    private(set) var isRunning = false
    private(set) var questConnected = false
    private(set) var pairCode: String?
    private(set) var pairDevice = ""
    private(set) var statsLine = ""
    private(set) var audioOK = false
    private(set) var gameStreaming = false
    private(set) var faceTrackingActive = false
    private(set) var lastError: String?
    private(set) var logs: [String] = []
    private(set) var runtimeInfo: VrRuntimeInfo
    private(set) var games: [VRGameItem] = []
    private(set) var isScanningGames = false

    var preset: VRQualityPreset {
        didSet {
            defaults.set(preset.rawValue, forKey: Keys.preset)
            if let p = preset.parameters {
                applyingPreset = true
                eye = p.eye
                fps = p.fps
                mbps = p.mbps
                foveation = p.foveation
                applyingPreset = false
            }
        }
    }

    var eye: Int {
        didSet {
            defaults.set(eye, forKey: Keys.eye)
            if !applyingPreset { preset = .custom }
        }
    }

    var fps: Int {
        didSet {
            defaults.set(fps, forKey: Keys.fps)
            if !applyingPreset { preset = .custom }
        }
    }

    var mbps: Int {
        didSet {
            defaults.set(mbps, forKey: Keys.mbps)
            if !applyingPreset { preset = .custom }
        }
    }

    var codec: VRCodec {
        didSet { defaults.set(codec.rawValue, forKey: Keys.codec) }
    }

    var foveation: VRFoveation {
        didSet {
            defaults.set(foveation.rawValue, forKey: Keys.foveation)
            if !applyingPreset { preset = .custom }
        }
    }

    /// Global VR render scale percentage (50...100). Games render at `eye * renderScale / 100`
    /// and ObeliskVRRenderer upscales back to `eye` using MetalFX Spatial.
    var renderScale: Int {
        didSet {
            let clamped = min(100, max(50, renderScale))
            if clamped != renderScale { renderScale = clamped; return }
            defaults.set(clamped, forKey: Keys.renderScale)
            persistGlobalRenderScale(clamped)
        }
    }

    /// Effective per-eye resolution the game will render before MetalFX Spatial upscaling.
    var scaledEyeResolution: Int {
        max(256, Int((Double(eye) * Double(renderScale) / 100.0 / 8.0).rounded()) * 8)
    }

    var audioRouting: VRAudioRouting {
        didSet { defaults.set(audioRouting.rawValue, forKey: Keys.audioRouting) }
    }

    var cableOnly: Bool {
        didSet { defaults.set(cableOnly, forKey: Keys.cableOnly) }
    }

    var showHeadsetHud: Bool {
        didSet {
            defaults.set(showHeadsetHud, forKey: Keys.showHeadsetHud)
            persistHeadsetHudToggle(showHeadsetHud)
        }
    }

    var faceTrackingOSC: Bool {
        didSet {
            defaults.set(faceTrackingOSC, forKey: Keys.faceTrackingOSC)
            persistFaceTrackingToggle(faceTrackingOSC)
        }
    }

    var frameGenMode: VRFrameGenMode {
        didSet {
            defaults.set(frameGenMode.rawValue, forKey: Keys.frameGenMode)
            persistFrameGenMode(frameGenMode)
        }
    }

    var useQuestMic: Bool {
        didSet { defaults.set(useQuestMic, forKey: Keys.useQuestMic) }
    }

    var customSteamVRPath: String {
        didSet {
            defaults.set(customSteamVRPath, forKey: Keys.steamVRPath)
            persistSteamVRPath()
            refreshRuntimeInfo()
        }
    }

    /// True when the active stream was started with different parameters than currently chosen.
    var needsRestartToApply: Bool {
        isRunning && runningSignature != currentSignature
    }

    @ObservationIgnored var onRequestLaunch: (@MainActor (String) -> Void)?
    @ObservationIgnored var onRequestQuit: (@MainActor () -> Void)?

    @ObservationIgnored private let locations: ObeliskLocations
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private var rendererProcess: Process?
    @ObservationIgnored private var networkBridge: VRNetworkBridge?
    @ObservationIgnored private var partialOutput = ""
    @ObservationIgnored private var stopRequested = false
    @ObservationIgnored private var restartAfterStop = false
    @ObservationIgnored private var pendingLaunchAfterRestart: String?
    @ObservationIgnored private var runningSignature = ""
    @ObservationIgnored private var applyingPreset = false
    /// Preserves the Quest headset's exact canted optical lens center & foveation center size
    /// (`"size,cxL,cxR,cy"`, e.g. `"2,0.6213,0.3787,0.3955"`) reported by the Quest APK.
    @ObservationIgnored private var foveationCenterSuffix: String?

    private enum Keys {
        static let preset = "obelisk.vr.preset"
        static let eye = "obelisk.vr.eye"
        static let fps = "obelisk.vr.fps"
        static let mbps = "obelisk.vr.mbps"
        static let codec = "obelisk.vr.codec"
        static let foveation = "obelisk.vr.foveation"
        static let foveationCenter = "obelisk.vr.foveationCenter"
        static let renderScale = "obelisk.vr.renderScale"
        static let audioRouting = "obelisk.vr.audioRouting"
        static let cableOnly = "obelisk.vr.cableOnly"
        static let showHeadsetHud = "obelisk.vr.showHeadsetHud"
        static let hudToggleRev = "obelisk.vr.hudToggleRev"
        static let faceTrackingOSC = "obelisk.vr.faceTrackingOSC"
        static let frameGenMode = "obelisk.vr.frameGenMode"
        static let frameGenRev = "obelisk.vr.frameGenRev"
        static let useQuestMic = "obelisk.vr.useQuestMic"
        static let steamVRPath = "obelisk.vr.steamVRPath"
    }

    private var currentSignature: String {
        "\(eye)-\(fps)-\(mbps)-\(codec.rawValue)-\(foveation.envValue(centerSuffix: foveationCenterSuffix))-\(audioRouting.rawValue)-\(cableOnly)-\(useQuestMic)"
    }

    var dataDirectory: URL { URL(fileURLWithPath: locations.vrData) }

    init(locations: ObeliskLocations = .live, defaults: UserDefaults = .standard) {
        self.locations = locations
        self.defaults = defaults
        let savedPreset = VRQualityPreset(rawValue: defaults.string(forKey: Keys.preset) ?? "") ?? .medium
        preset = savedPreset
        let defaultParams = savedPreset.parameters ?? (1440, 90, 80, .light)
        let storedEye = defaults.integer(forKey: Keys.eye)
        eye = storedEye > 0 ? storedEye : defaultParams.eye
        let storedFps = defaults.integer(forKey: Keys.fps)
        fps = storedFps > 0 ? storedFps : defaultParams.fps
        let storedMbps = defaults.integer(forKey: Keys.mbps)
        mbps = storedMbps > 0 ? storedMbps : defaultParams.mbps
        codec = VRCodec(rawValue: defaults.string(forKey: Keys.codec) ?? "") ?? .hevc
        foveation = VRFoveation(rawValue: defaults.string(forKey: Keys.foveation) ?? "") ?? defaultParams.foveation
        foveationCenterSuffix = defaults.string(forKey: Keys.foveationCenter)
        let storedScale = defaults.integer(forKey: Keys.renderScale)
        let profileScale = Self.readGlobalRenderScale(in: URL(fileURLWithPath: locations.vrData))
        let resolvedScale = profileScale ?? (storedScale >= 50 && storedScale <= 100 ? storedScale : 100)
        renderScale = resolvedScale
        audioRouting = VRAudioRouting(rawValue: defaults.string(forKey: Keys.audioRouting) ?? "") ?? .quest
        cableOnly = defaults.bool(forKey: Keys.cableOnly)
        showHeadsetHud = defaults.bool(forKey: Keys.showHeadsetHud)
        faceTrackingOSC = defaults.object(forKey: Keys.faceTrackingOSC) as? Bool ?? true
        frameGenMode = VRFrameGenMode(rawValue: defaults.integer(forKey: Keys.frameGenMode)) ?? .off
        useQuestMic = defaults.bool(forKey: Keys.useQuestMic)
        let savedSteamVRPath = defaults.string(forKey: Keys.steamVRPath) ?? ""
        customSteamVRPath = savedSteamVRPath

        if AppIdentity.isTestHost {
            runtimeInfo = VrRuntimeInfo(
                rendererBundled: false,
                openxrBundled: false,
                opencompositeBundled: false,
                steamvrDriverBundled: false,
                steamvrPath: nil
            )
        } else {
            runtimeInfo = vrRuntimeInfo(
                locations: locations,
                customSteamvrPath: savedSteamVRPath.isEmpty ? nil : savedSteamVRPath
            )
            persistSteamVRPath()
            clearRendererPidFile()
            startNetworkBridge()
        }
    }

    func refreshRuntimeInfo() {
        guard !AppIdentity.isTestHost else { return }
        let loc = locations
        let custom = customSteamVRPath.isEmpty ? nil : customSteamVRPath
        Task {
            let latest = await Task.detached(priority: .utility) {
                vrRuntimeInfo(locations: loc, customSteamvrPath: custom)
            }.value
            runtimeInfo = latest
        }
    }

    /// Scans installed Windows games from all Obelisk storefronts for VR APIs and
    /// writes `obelisk-games.json` so the Quest headset's in-VR library stays in sync.
    func refreshGames(from entries: [(id: String, title: String, storefront: LibrarySection, directory: String)]) {
        guard !AppIdentity.isTestHost else { return }
        isScanningGames = true
        let dataDir = dataDirectory
        Task {
            let scanned: [VRGameItem] = await Task.detached(priority: .utility) {
                var results = entries.map { entry in
                    VRGameItem(
                        id: entry.id,
                        title: entry.title,
                        storefront: entry.storefront,
                        directory: entry.directory,
                        detection: detectGameVr(installPath: entry.directory)
                    )
                }
                results.sort { lhs, rhs in
                    if lhs.detection.isVr != rhs.detection.isVr {
                        return lhs.detection.isVr && !rhs.detection.isVr
                    }
                    return lhs.title.localizedCaseInsensitiveCompare(rhs.title) == .orderedAscending
                }
                Self.writeQuestCatalog(results, to: dataDir)
                return results
            }.value
            games = scanned
            isScanningGames = false
        }
    }

    nonisolated private static func writeQuestCatalog(_ items: [VRGameItem], to dataDir: URL) {
        try? FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        let payload: [[String: Any]] = items.map { item in
            [
                "id": item.id,
                "name": item.title,
                "vr": item.detection.isVr,
                "dir": item.folderName,
            ]
        }
        guard let json = try? JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted]) else { return }
        let target = dataDir.appendingPathComponent("obelisk-games.json")
        try? json.write(to: target, options: .atomic)
    }

    func startServer() {
        guard !AppIdentity.isTestHost, rendererProcess == nil else { return }
        let binary = locations.vrRenderer
        guard FileManager.default.isExecutableFile(atPath: binary) else {
            lastError = L10n.string("ui.vr.error.renderer_missing")
            return
        }

        lastError = nil
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)

        let process = Process()
        process.executableURL = URL(fileURLWithPath: binary)
        process.currentDirectoryURL = dataDirectory
        var env = ProcessInfo.processInfo.environment
        env["OBELISK_VR_HOST"] = "1"
        env["QVB_DATA"] = dataDirectory.path
        env["VR_EYE"] = String(eye)
        env["VR_FPS"] = String(fps)
        env["VR_MBPS"] = String(mbps)
        let fovEnv = foveation.envValue(centerSuffix: foveationCenterSuffix)
        env["VR_CODEC"] = codec.rawValue
        env["VR_FOV"] = fovEnv
        env["VR_GEN_EYE"] = String(eye)
        env["VR_GEN_FPS"] = String(fps)
        env["VR_GEN_MBPS"] = String(mbps)
        env["VR_GEN_FOV"] = fovEnv
        env["VR_AUDIO"] = audioRouting == .mac ? "0" : "1"
        env["VR_AUDIO_MAC"] = audioRouting == .quest ? "0" : "1"
        env["VR_CABLE_ONLY"] = cableOnly ? "1" : "0"
        env["VR_FACE_TRACKING"] = faceTrackingOSC ? "1" : "0"
        env["VR_MIC"] = useQuestMic ? "quest" : ""
        env["VRMIN_LANG"] = L10n.languageCode == "es" ? "es" : "en"
        process.environment = env
        persistFaceTrackingToggle(faceTrackingOSC)
        if frameGenMode != .off { persistFrameGenMode(frameGenMode) }

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                return
            }
            let chunk = String(decoding: data, as: UTF8.self)
            Task { @MainActor [weak self] in
                self?.ingest(chunk)
            }
        }

        stopRequested = false
        process.terminationHandler = { [weak self] finished in
            let code = finished.terminationStatus
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.rendererProcess = nil
                self.isRunning = false
                self.clearRendererPidFile()
                self.questConnected = false
                self.gameStreaming = false
                self.faceTrackingActive = false
                self.statsLine = ""
                self.audioOK = false
                self.pairCode = nil
                self.networkBridge?.setServerRunning(false)
                if code != 0 && !self.stopRequested {
                    self.lastError = L10n.string("ui.vr.error.renderer_exited", Int(code))
                }
                if self.restartAfterStop {
                    self.restartAfterStop = false
                    let launchId = self.pendingLaunchAfterRestart
                    self.pendingLaunchAfterRestart = nil
                    self.startServer()
                    if let launchId {
                        self.onRequestLaunch?(launchId)
                    }
                }
            }
        }

        do {
            try process.run()
            rendererProcess = process
            isRunning = true
            writeRendererPidFile(process.processIdentifier)
            runningSignature = currentSignature
            networkBridge?.setServerRunning(true)
            appendLog("VR renderer started (\(eye)x\(eye) @ \(fps) Hz · \(mbps) Mbps · \(codec.rawValue.uppercased()))")
        } catch {
            lastError = error.localizedDescription
        }
    }

    func ensureRunning() {
        if !isRunning {
            startServer()
        }
    }

    func restartServer() {
        guard rendererProcess != nil else {
            startServer()
            return
        }
        restartAfterStop = true
        stopServer()
    }

    func stopServer(wait: Bool = false) {
        clearRendererPidFile()
        guard let process = rendererProcess else { return }
        stopRequested = true
        process.terminate()
        if wait {
            for _ in 0..<25 where process.isRunning {
                Thread.sleep(forTimeInterval: 0.05)
            }
            if process.isRunning {
                kill(process.processIdentifier, SIGKILL)
            }
        }
    }

    func shutdown() {
        networkBridge?.stop()
        networkBridge = nil
        stopServer(wait: true)
    }

    func openDataFolder() {
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        NSWorkspace.shared.open(dataDirectory)
    }

    func chooseSteamVRFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.message = L10n.string("ui.vr.steamvr.pick_message")
        panel.prompt = L10n.string("ui.vr.steamvr.pick_confirm")
        if panel.runModal() == .OK, let url = panel.url {
            customSteamVRPath = url.path
        }
    }

    func clearCustomSteamVRPath() {
        customSteamVRPath = ""
    }

    private func writeRendererPidFile(_ pid: Int32) {
        guard !AppIdentity.isTestHost else { return }
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        let file = dataDirectory.appendingPathComponent("renderer.pid")
        try? "\(pid)\n".write(to: file, atomically: true, encoding: .utf8)
    }

    private func clearRendererPidFile() {
        guard !AppIdentity.isTestHost else { return }
        let file = dataDirectory.appendingPathComponent("renderer.pid")
        try? FileManager.default.removeItem(at: file)
    }

    private func persistSteamVRPath() {
        guard !AppIdentity.isTestHost else { return }
        let file = dataDirectory.appendingPathComponent("steamvr-path.txt")
        if customSteamVRPath.isEmpty {
            try? FileManager.default.removeItem(at: file)
        } else {
            try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
            try? customSteamVRPath.write(to: file, atomically: true, encoding: .utf8)
        }
    }

    private static func readGlobalRenderScale(in dir: URL) -> Int? {
        let file = dir.appendingPathComponent("perfiles_juegos.json")
        guard let data = try? Data(contentsOf: file),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let global = root[""] as? [String: Any],
              let scale = global["scale"] as? Double else { return nil }
        let pct = Int((scale * 100.0).rounded())
        return (50...100).contains(pct) ? pct : nil
    }

    private func persistGlobalRenderScale(_ percent: Int) {
        guard !AppIdentity.isTestHost else { return }
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        let file = dataDirectory.appendingPathComponent("perfiles_juegos.json")
        var root = ((try? Data(contentsOf: file))
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }) ?? [:]
        var global = (root[""] as? [String: Any]) ?? [:]
        global["scale"] = Double(percent) / 100.0
        root[""] = global
        if let data = try? JSONSerialization.data(withJSONObject: root, options: [.sortedKeys]) {
            try? data.write(to: file, options: .atomic)
        }
    }

    private func persistHeadsetHudToggle(_ on: Bool) {
        guard !AppIdentity.isTestHost else { return }
        let rev = defaults.integer(forKey: Keys.hudToggleRev) + 1
        defaults.set(rev, forKey: Keys.hudToggleRev)
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        let file = dataDirectory.appendingPathComponent("hud-toggle.txt")
        try? "\(on ? 1 : 0)\t\(rev)\n".write(to: file, atomically: true, encoding: .utf8)
    }

    private func persistFaceTrackingToggle(_ on: Bool) {
        guard !AppIdentity.isTestHost else { return }
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        let file = dataDirectory.appendingPathComponent("ft-toggle.txt")
        try? "\(on ? 1 : 0)\n".write(to: file, atomically: true, encoding: .utf8)
    }

    private func persistFrameGenMode(_ mode: VRFrameGenMode) {
        guard !AppIdentity.isTestHost else { return }
        let rev = defaults.integer(forKey: Keys.frameGenRev) + 1
        defaults.set(rev, forKey: Keys.frameGenRev)
        try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        let file = dataDirectory.appendingPathComponent("framegen-toggle.txt")
        try? "\(mode.rawValue)\t\(rev)\n".write(to: file, atomically: true, encoding: .utf8)
    }

    private func updateFoveation(from envValue: String, ignorePlaceholderWhenOff: Bool = false) {
        if let suffix = VRFoveation.centerSuffix(from: envValue) {
            foveationCenterSuffix = suffix
            defaults.set(suffix, forKey: Keys.foveationCenter)
        } else if ignorePlaceholderWhenOff && foveation == .off {
            // Never let a stale placeholder per-game profile ("2,1,0.5,0.5,0.5") silently re-enable foveation.
            return
        }
        foveation = VRFoveation.from(envValue: envValue)
    }

    private func startNetworkBridge() {
        let hostName = (Host.current().localizedName ?? "Mac").replacingOccurrences(of: "\"", with: "")
        let handlers = VRNetworkBridge.Handlers(
            onWakeRequest: { [weak self] in
                Task { @MainActor [weak self] in
                    guard let self, !self.isRunning else { return }
                    self.appendLog("Quest requested connection — starting VR renderer")
                    self.startServer()
                }
            },
            onSettingsUpdate: { [weak self] eye, fps, mbps, codec, audio, fov in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    if let eye, (512...2560).contains(eye) { self.eye = eye }
                    if let fps, Self.fpsOptions.contains(fps) { self.fps = fps }
                    if let mbps, (10...300).contains(mbps) { self.mbps = mbps }
                    if let codec, let c = VRCodec(rawValue: codec) { self.codec = c }
                    if let audio, let a = VRAudioRouting(rawValue: audio) { self.audioRouting = a }
                    if let fov { self.updateFoveation(from: fov) }
                    if self.isRunning { self.restartServer() }
                }
            },
            onGameConfigUpdate: { [weak self] scale in
                Task { @MainActor [weak self] in
                    guard let self, let scale, (50...100).contains(scale) else { return }
                    if self.renderScale != scale {
                        self.renderScale = scale
                    }
                }
            },
            onPlayGameWithQuality: { [weak self] id, eye, fps, mbps, fov in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.eye = eye
                    self.fps = fps
                    self.mbps = mbps
                    self.updateFoveation(from: fov, ignorePlaceholderWhenOff: true)
                    self.pendingLaunchAfterRestart = id
                    self.restartServer()
                }
            },
            onLaunchGame: { [weak self] id in
                Task { @MainActor [weak self] in
                    self?.onRequestLaunch?(id)
                }
            },
            onQuitGame: { [weak self] in
                Task { @MainActor [weak self] in
                    self?.onRequestQuit?()
                }
            }
        )
        let bridge = VRNetworkBridge(macName: hostName, handlers: handlers)
        bridge.start()
        networkBridge = bridge
    }

    private func ingest(_ text: String) {
        let combined = (partialOutput + text).replacingOccurrences(of: "\r", with: "\n")
        var lines = combined.components(separatedBy: "\n")
        partialOutput = lines.removeLast()
        for raw in lines {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { continue }
            handleRendererLine(line)
        }
    }

    private func handleRendererLine(_ line: String) {
        if let range = line.range(of: "[emparejar] ") {
            let parts = line[range.upperBound...].split(separator: " ", maxSplits: 2).map(String.init)
            if parts.first == "CODIGO", parts.count >= 2 {
                pairCode = parts[1]
                pairDevice = parts.count > 2 ? parts[2] : "Quest"
            } else if parts.first == "LISTO" || parts.first == "VENCIDO" {
                pairCode = nil
            }
            appendLog(line)
            return
        }
        if line.contains("[+] Quest conectado") || line.contains("[quest] App del Quest conectada") {
            questConnected = true
        } else if line.contains("[-] Quest desconectado") || line.contains("[quest] App del Quest desconectada") {
            questConnected = false
            gameStreaming = false
            faceTrackingActive = false
        } else if line.contains("[vrcft] Quest Pro Eye & Face Tracking active") {
            faceTrackingActive = true
        } else if line.contains("[juego] Primer cuadro recibido") {
            gameStreaming = true
        } else if line.hasPrefix("Quest ") && line.contains("poses/s") {
            statsLine = line
            audioOK = line.contains("sonido OK")
            return
        }
        appendLog(line)
    }

    private func appendLog(_ line: String) {
        logs.append(line)
        if logs.count > 400 {
            logs.removeFirst(logs.count - 400)
        }
    }
}
