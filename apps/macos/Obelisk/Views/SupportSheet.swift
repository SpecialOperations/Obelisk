import AppKit
import SwiftUI

/// The ways to support Obelisk FX, each a card: a star on the fork's GitHub
/// repository, reporting issues on the fork (never upstream), and copying the
/// repository link to share, with clear attribution to upstream Obelisk.
struct SupportSheet: View {
    let model: SupportModel
    let close: () -> Void
    /// Whether the link was copied since the sheet opened.
    @State private var copiedLink = false

    private static let tealColors = [Color(red: 0.09, green: 0.68, blue: 0.72), Color(red: 0.04, green: 0.42, blue: 0.47)]
    private static let coralColors = [Color(red: 1.0, green: 0.36, blue: 0.32), Color(red: 0.90, green: 0.22, blue: 0.34)]

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            SheetHeader(title: L10n.string("ui.support.title", AppIdentity.name),
                        subtitle: L10n.string("ui.support.message", AppIdentity.name),
                        symbol: "heart.fill", colors: TileColors.red, close: close, closeIdentifier: "support-close")
            VStack(spacing: 10) {
                SupportWay(title: L10n.string("ui.support.star"),
                           detail: L10n.string("ui.support.star_description", AppIdentity.name),
                           identifier: "support-star") {
                    NSWorkspace.shared.open(AppIdentity.repository)
                } icon: {
                    LogoTile(asset: "GitHubLogo", colors: TileColors.graphite, size: 36)
                } accessory: {
                    if let stars = model.stars {
                        SupportCount(symbol: "star.fill", count: stars)
                            .transition(.opacity)
                    }
                    Image(systemName: "arrow.up.right")
                }
                .accessibilityValue(model.stars.map { L10n.string("ui.support.stars", Int64($0)) } ?? "")
                SupportWay(title: L10n.string("ui.support.issues"),
                           detail: L10n.string("ui.support.issues_description", AppIdentity.name),
                           identifier: "support-issues") {
                    NSWorkspace.shared.open(AppIdentity.repository.appending(path: "issues"))
                } icon: {
                    SymbolTile(name: "ladybug.fill", colors: Self.coralColors, size: 36)
                } accessory: {
                    Image(systemName: "arrow.up.right")
                }
                SupportWay(title: L10n.string("ui.support.share"),
                           detail: L10n.string("ui.support.share_description"),
                           identifier: "support-share") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(SupportModel.site.absoluteString, forType: .string)
                    copiedLink = true
                } icon: {
                    SymbolTile(name: "square.and.arrow.up", colors: Self.tealColors, size: 36)
                } accessory: {
                    if copiedLink {
                        Text(L10n.string("ui.support.link_copied"))
                            .transition(.opacity)
                    }
                    Image(systemName: copiedLink ? "checkmark" : "doc.on.doc")
                }
                .accessibilityValue(copiedLink ? L10n.string("ui.support.link_copied") : "")
            }
            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch")
                    .font(.system(size: 11))
                Text(L10n.string("ui.support.fork_credit", AppIdentity.name))
                Spacer(minLength: 0)
                Button(L10n.string("ui.settings.fork_upstream")) {
                    NSWorkspace.shared.open(AppIdentity.upstreamRepository)
                }
                .obeliskButtonStyle(.plain)
                .foregroundStyle(ObeliskStyle.accent)
            }
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 4)
        }
        .padding(24)
        .frame(width: 480)
        .tint(ObeliskStyle.accent)
        .animation(.easeInOut(duration: 0.2), value: model.stars)
        .animation(.easeInOut(duration: 0.2), value: copiedLink)
        .accessibilityIdentifier("support-sheet")
    }
}

/// A way to support Obelisk, as a card to press: its tile, what it is over
/// what it brings, then what pressing it does.
private struct SupportWay<Icon: View, Accessory: View>: View {
    let title: String
    let detail: String
    let identifier: String
    let action: () -> Void
    @ViewBuilder let icon: Icon
    @ViewBuilder let accessory: Accessory

    var body: some View {
        ChoiceCard(identifier: identifier, action: action) {
            ObeliskRow(title: title) {
                icon
            } detail: {
                Text(detail)
            } accessory: {
                HStack(spacing: 8) { accessory }
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .multilineTextAlignment(.leading)
        }
        .accessibilityLabel(title)
        .accessibilityHint(detail)
    }
}

/// How many stars or members a way has, on the gradient getobelisk.app shows
/// them on.
private struct SupportCount: View {
    let symbol: String
    let count: Int

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: symbol)
                .font(.system(size: 10, weight: .semibold))
            Text(count.formatted(.number.locale(L10n.locale)))
                .font(.system(size: 12, weight: .bold))
                .monospacedDigit()
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 9)
        .frame(height: 22)
        .background(LinearGradient(colors: [Color(red: 0.068, green: 0.56, blue: 0.60),
                                            Color(red: 0.96, green: 0.45, blue: 0.22)],
                                   startPoint: .leading, endPoint: .trailing), in: .capsule)
        .accessibilityHidden(true)
    }
}
