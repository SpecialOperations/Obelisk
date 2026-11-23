#!/bin/bash
# Obelisk FX's version order, as Sparkle compares CFBundleVersion: X.Y.Z or X.Y.Z.W, after its betas X.Y.Z(.W).beta<n>,
# such as 0.6.4 < 0.6.4.1.beta1 < 0.6.4.1 < 0.6.5. Legacy fork tags like v0.6.4-metalfx.3 or v0.6.4-fx.1 also map to
# the 4th component so write-appcast.sh can sign them, though new releases should use dotted X.Y.Z.W for CFBundleVersion
# because Sparkle ignores anything after a dash. Sourced by beta.sh and write-appcast.sh.

# A key that orders version or tag $1 as Sparkle orders it, compared as text; fails for any other name.
version_key() {
    local v="${1#v}" patch4=0 stage=1 beta=0
    if [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.([0-9]+)\.beta([0-9]+)$ ]]; then
        patch4="${BASH_REMATCH[4]}" stage=0 beta="${BASH_REMATCH[5]}"
    elif [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.beta([0-9]+)$ ]]; then
        stage=0 beta="${BASH_REMATCH[4]}"
    elif [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        patch4="${BASH_REMATCH[4]}"
    elif [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)-metalfx\.([0-9]+)$ ]]; then
        patch4="${BASH_REMATCH[4]}"
    elif [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)-fx\.([0-9]+)$ ]]; then
        patch4="${BASH_REMATCH[4]}"
    elif [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        :
    else
        return 1
    fi
    printf '%010d%010d%010d%010d%d%010d' "$((10#${BASH_REMATCH[1]}))" "$((10#${BASH_REMATCH[2]}))" \
        "$((10#${BASH_REMATCH[3]}))" "$((10#$patch4))" "$stage" "$((10#$beta))"
}
