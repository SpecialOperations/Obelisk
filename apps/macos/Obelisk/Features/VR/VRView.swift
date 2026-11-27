import AppKit
import SwiftUI

/// Dedicated VR hub inside Obelisk FX: manages headset streaming (Meta Quest via
/// the VRmin Quest APK protocol), SteamVR / OpenXR / OpenComposite runtime setup,
/// streaming quality, and installed VR games across all storefronts.
struct VRView: View {
    @Bindable var model: AppModel
    @State private var showAllInstalled = false
    @State private var showLogs = false

    private static let contentWidth: CGFloat = 900

    private var vr: VRModel { model.vr }

    private var visibleGames: [VRGameItem] {
        showAllInstalled ? vr.games : vr.games.filter { $0.detection.isVr }
    }

    var body: some View {
        StorefrontScrollView {
            VStack(alignment: .leading, spacing: 30) {
                header
                statusSection
                if let code = vr.pairCode {
                    pairingCard(code: code, device: vr.pairDevice)
                }
                gamesSection
                streamingSection
                runtimeSection
                attributionSection
            }
            .frame(maxWidth: Self.contentWidth, alignment: .leading)
            .padding(.horizontal, ObeliskStyle.contentPadding)
            .padding(.top, 28)
            .padding(.bottom, 40)
        }
        .onAppear {
            vr.refreshRuntimeInfo()
            model.syncVRGames()
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .center, spacing: 16) {
            SymbolTile(name: "visionpro", colors: TileColors.violet)
                .scaleEffect(1.15)
            VStack(alignment: .leading, spacing: 4) {
                Text(L10n.string("ui.vr.title"))
                    .font(.system(size: 26, weight: .bold))
                Text(L10n.string("ui.vr.subtitle"))
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            if vr.isRunning {
                GlassCapsuleButton(
                    title: L10n.string("ui.vr.stop_server"),
                    systemImage: "stop.fill",
                    identifier: "vr-stop-server"
                ) {
                    vr.stopServer()
                }
            } else {
                AccentCapsuleButton(
                    title: L10n.string("ui.vr.start_server"),
                    identifier: "vr-start-server"
                ) {
                    vr.startServer()
                }
            }
        }
    }

    // MARK: Status

    private var statusSection: some View {
        ObeliskCard {
            ObeliskRow(title: statusTitle) {
                SymbolTile(
                    name: vr.questConnected ? "checkmark.circle.fill" : (vr.isRunning ? "dot.radiowaves.left.and.right" : "wifi.slash"),
                    colors: vr.questConnected ? TileColors.green : (vr.isRunning ? TileColors.blue : TileColors.graphite)
                )
            } detail: {
                Text(statusDetail)
            } accessory: {
                if vr.needsRestartToApply {
                    AccentCapsuleButton(
                        title: L10n.string("ui.vr.apply_restart"),
                        identifier: "vr-apply-restart"
                    ) {
                        vr.restartServer()
                    }
                }
            }

            if !vr.statsLine.isEmpty {
                ObeliskCardDivider()
                HStack(spacing: 10) {
                    Image(systemName: "waveform.path.ecg")
                        .foregroundStyle(.secondary)
                    Text(vr.statsLine)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .textSelection(.enabled)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            }

            if let error = vr.lastError {
                ObeliskCardDivider()
                HStack(spacing: 10) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                    Text(error)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            }
        }
    }

    private var statusTitle: String {
        if vr.questConnected {
            return vr.gameStreaming
                ? L10n.string("ui.vr.status.streaming")
                : L10n.string("ui.vr.status.connected")
        }
        return vr.isRunning
            ? L10n.string("ui.vr.status.waiting")
            : L10n.string("ui.vr.status.stopped")
    }

    private var statusDetail: String {
        if vr.questConnected {
            return L10n.string("ui.vr.status.connected_detail", vr.eye, vr.fps, vr.mbps)
        }
        if vr.isRunning {
            return L10n.string("ui.vr.status.waiting_detail")
        }
        return L10n.string("ui.vr.status.stopped_detail")
    }

    private func pairingCard(code: String, device: String) -> some View {
        ObeliskCard {
            ObeliskRow(title: L10n.string("ui.vr.pair.title", device)) {
                SymbolTile(name: "key.fill", colors: TileColors.gold)
            } detail: {
                Text(L10n.string("ui.vr.pair.detail"))
            } accessory: {
                Text(code)
                    .font(.system(size: 24, weight: .bold, design: .monospaced))
                    .foregroundStyle(ObeliskStyle.accent)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                    .background(ObeliskStyle.accent.opacity(0.14), in: RoundedRectangle(cornerRadius: 10))
                    .textSelection(.enabled)
            }
        }
    }

    // MARK: Installed VR Games

    private var gamesSection: some View {
        GamePageSection(title: L10n.string("ui.vr.games.section")) {
            ObeliskCard {
                HStack {
                    Text(L10n.string("ui.vr.games.description"))
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                    Spacer(minLength: 8)
                    Toggle(L10n.string("ui.vr.games.show_all"), isOn: $showAllInstalled)
                        .toggleStyle(.switch)
                        .controlSize(.small)
                    GlassCapsuleButton(
                        title: L10n.string("ui.vr.games.rescan"),
                        systemImage: "arrow.clockwise",
                        identifier: "vr-rescan-games"
                    ) {
                        model.syncVRGames()
                    }
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)

                if vr.isScanningGames && vr.games.isEmpty {
                    ObeliskCardDivider()
                    HStack(spacing: 10) {
                        ProgressView().controlSize(.small)
                        Text(L10n.string("ui.vr.games.scanning"))
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                    }
                    .padding(18)
                } else if visibleGames.isEmpty {
                    ObeliskCardDivider()
                    Text(L10n.string("ui.vr.games.empty"))
                        .font(.system(size: 13))
                        .foregroundStyle(.secondary)
                        .padding(18)
                } else {
                    ForEach(visibleGames) { item in
                        ObeliskCardDivider()
                        ObeliskRow(title: item.title) {
                            SymbolTile(
                                name: item.detection.isVr ? "visionpro.fill" : "gamecontroller.fill",
                                colors: item.detection.isVr ? TileColors.violet : TileColors.graphite
                            )
                        } detail: {
                            Text(gameSubtitle(for: item))
                        } accessory: {
                            HStack(spacing: 8) {
                                GlassCapsuleButton(
                                    title: L10n.string("ui.vr.games.open"),
                                    systemImage: "slider.horizontal.3",
                                    identifier: "vr-open-\(item.id)"
                                ) {
                                    model.openVRGame(id: item.id)
                                }
                                AccentCapsuleButton(
                                    title: L10n.string("ui.vr.games.play"),
                                    identifier: "vr-play-\(item.id)"
                                ) {
                                    model.launchVRGame(id: item.id)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func gameSubtitle(for item: VRGameItem) -> String {
        var tags: [String] = [item.storefront.title]
        if item.detection.hasOpenxr { tags.append("OpenXR") }
        if item.detection.hasOpenvr { tags.append("OpenVR") }
        if item.detection.hasOculus { tags.append("Oculus") }
        switch item.detection.engine {
        case .unity: tags.append("Unity")
        case .unreal: tags.append("Unreal")
        case .unknown: break
        }
        return tags.joined(separator: " · ")
    }

    // MARK: Streaming Quality & Audio

    private var streamingSection: some View {
        GamePageSection(title: L10n.string("ui.vr.streaming.section")) {
            ObeliskCard {
                ObeliskRow(title: L10n.string("ui.vr.streaming.preset")) {
                    SymbolTile(name: "sparkles", colors: TileColors.violet)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.preset_detail"))
                } accessory: {
                    GlassMenuPicker(
                        title: L10n.string("ui.vr.streaming.preset"),
                        value: vr.preset.title,
                        selection: Binding(get: { vr.preset }, set: { vr.preset = $0 }),
                        identifier: "vr-preset"
                    ) {
                        ForEach(VRQualityPreset.allCases) { preset in
                            Text(preset.title).tag(preset)
                        }
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.resolution")) {
                    SymbolTile(name: "viewfinder", colors: TileColors.blue)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.resolution_detail"))
                } accessory: {
                    GlassMenuPicker(
                        title: L10n.string("ui.vr.streaming.resolution"),
                        value: L10n.string("ui.vr.format.resolution", vr.eye, vr.eye),
                        selection: Binding(get: { vr.eye }, set: { vr.eye = $0 }),
                        identifier: "vr-eye"
                    ) {
                        ForEach(VRModel.eyeOptions, id: \.self) { size in
                            Text(L10n.string("ui.vr.format.resolution", size, size)).tag(size)
                        }
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.fps")) {
                    SymbolTile(name: "gauge.with.needle.fill", colors: TileColors.green)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.fps_detail"))
                } accessory: {
                    GlassMenuPicker(
                        title: L10n.string("ui.vr.streaming.fps"),
                        value: L10n.string("ui.vr.format.hz", vr.fps),
                        selection: Binding(get: { vr.fps }, set: { vr.fps = $0 }),
                        identifier: "vr-fps"
                    ) {
                        ForEach(VRModel.fpsOptions, id: \.self) { rate in
                            Text(L10n.string("ui.vr.format.hz", rate)).tag(rate)
                        }
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.bitrate")) {
                    SymbolTile(name: "speedometer", colors: TileColors.blue)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.bitrate_detail", vr.mbps))
                } accessory: {
                    HStack(spacing: 10) {
                        Slider(
                            value: Binding(get: { Double(vr.mbps) }, set: { vr.mbps = Int($0.rounded()) }),
                            in: 20...200,
                            step: 5
                        )
                        .frame(width: 150)
                        Text(L10n.string("ui.vr.format.mbps", vr.mbps))
                            .font(.system(size: 12, weight: .medium).monospacedDigit())
                            .foregroundStyle(.secondary)
                            .frame(width: 64, alignment: .trailing)
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.codec")) {
                    SymbolTile(name: "film", colors: TileColors.graphite)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.codec_detail"))
                } accessory: {
                    GlassMenuPicker(
                        title: L10n.string("ui.vr.streaming.codec"),
                        value: vr.codec.title,
                        selection: Binding(get: { vr.codec }, set: { vr.codec = $0 }),
                        identifier: "vr-codec"
                    ) {
                        ForEach(VRCodec.allCases) { codec in
                            Text(codec.title).tag(codec)
                        }
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.foveation")) {
                    SymbolTile(name: "eye", colors: TileColors.violet)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.foveation_detail"))
                } accessory: {
                    GlassMenuPicker(
                        title: L10n.string("ui.vr.streaming.foveation"),
                        value: vr.foveation.title,
                        selection: Binding(get: { vr.foveation }, set: { vr.foveation = $0 }),
                        identifier: "vr-foveation"
                    ) {
                        ForEach(VRFoveation.allCases) { level in
                            Text(level.title).tag(level)
                        }
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.audio")) {
                    SymbolTile(name: "speaker.wave.2.fill", colors: TileColors.green)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.audio_detail"))
                } accessory: {
                    GlassMenuPicker(
                        title: L10n.string("ui.vr.streaming.audio"),
                        value: vr.audioRouting.title,
                        selection: Binding(get: { vr.audioRouting }, set: { vr.audioRouting = $0 }),
                        identifier: "vr-audio"
                    ) {
                        ForEach(VRAudioRouting.allCases) { mode in
                            Text(mode.title).tag(mode)
                        }
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.streaming.cable_only")) {
                    SymbolTile(name: "cable.connector", colors: TileColors.graphite)
                } detail: {
                    Text(L10n.string("ui.vr.streaming.cable_only_detail"))
                } accessory: {
                    Toggle(L10n.string("ui.vr.streaming.cable_only"),
                           isOn: Binding(get: { vr.cableOnly }, set: { vr.cableOnly = $0 }))
                        .toggleStyle(.switch)
                        .labelsHidden()
                        .pointerCursor()
                }
            }
        }
    }

    // MARK: SteamVR & Bundled Runtimes

    private var runtimeSection: some View {
        GamePageSection(title: L10n.string("ui.vr.runtime.section")) {
            ObeliskCard {
                ObeliskRow(title: L10n.string("ui.vr.runtime.bundled")) {
                    SymbolTile(
                        name: bundledReady ? "checkmark.seal.fill" : "exclamationmark.triangle.fill",
                        colors: bundledReady ? TileColors.green : TileColors.gold
                    )
                } detail: {
                    Text(bundledReady
                         ? L10n.string("ui.vr.runtime.bundled_ready")
                         : L10n.string("ui.vr.runtime.bundled_missing"))
                } accessory: {
                    GlassCapsuleButton(
                        title: L10n.string("ui.vr.runtime.open_logs"),
                        systemImage: "folder",
                        identifier: "vr-open-data"
                    ) {
                        vr.openDataFolder()
                    }
                }

                ObeliskCardDivider()

                ObeliskRow(title: L10n.string("ui.vr.runtime.steamvr")) {
                    SymbolTile(
                        name: vr.runtimeInfo.steamvrPath != nil ? "checkmark.circle.fill" : "shippingbox",
                        colors: vr.runtimeInfo.steamvrPath != nil ? TileColors.green : TileColors.blue
                    )
                } detail: {
                    Text(vr.runtimeInfo.steamvrPath
                         ?? L10n.string("ui.vr.runtime.steamvr_missing"))
                        .lineLimit(2)
                } accessory: {
                    HStack(spacing: 8) {
                        if !vr.customSteamVRPath.isEmpty {
                            GlassCapsuleButton(
                                title: L10n.string("ui.vr.runtime.clear"),
                                systemImage: "xmark",
                                identifier: "vr-clear-steamvr"
                            ) {
                                vr.clearCustomSteamVRPath()
                            }
                        }
                        GlassCapsuleButton(
                            title: L10n.string("ui.vr.runtime.choose_folder"),
                            systemImage: "folder.badge.plus",
                            identifier: "vr-choose-steamvr"
                        ) {
                            vr.chooseSteamVRFolder()
                        }
                    }
                }

                ObeliskCardDivider()

                DisclosureGroup(L10n.string("ui.vr.runtime.live_log"), isExpanded: $showLogs) {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 2) {
                            ForEach(Array(vr.logs.enumerated()), id: \.offset) { _, line in
                                Text(line)
                                    .font(.system(size: 11, design: .monospaced))
                                    .foregroundStyle(.secondary)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                        .padding(10)
                    }
                    .frame(height: 180)
                    .background(Color.black.opacity(0.25), in: RoundedRectangle(cornerRadius: 8))
                    .padding(.top, 8)
                }
                .padding(.horizontal, 18)
                .padding(.vertical, 12)
            }
        }
    }

    private var bundledReady: Bool {
        vr.runtimeInfo.rendererBundled
            && vr.runtimeInfo.openxrBundled
            && vr.runtimeInfo.opencompositeBundled
            && vr.runtimeInfo.steamvrDriverBundled
    }

    // MARK: Attribution (VRmin GPL-3.0 Section 7)

    private var attributionSection: some View {
        ObeliskCard {
            ObeliskRow(title: L10n.string("ui.vr.attribution.title")) {
                SymbolTile(name: "doc.text.fill", colors: TileColors.graphite)
            } detail: {
                Text(L10n.string("ui.vr.attribution.detail"))
            } accessory: {
                GlassCapsuleButton(
                    title: L10n.string("ui.vr.attribution.github"),
                    systemImage: "arrow.up.right",
                    identifier: "vr-attribution-link"
                ) {
                    NSWorkspace.shared.open(VRModel.vrminRepository)
                }
            }
        }
    }
}
