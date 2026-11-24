#!/usr/bin/env bash
# Read-only snapshot. Pass a label and locally verified Deck nodes.
# Discovery and inspection do not grab input devices.
set -eu
export LC_ALL=C
shopt -s nullglob

usage() {
    printf 'Usage: %s                       # device discovery\n' "$0"
    printf '       %s LABEL DEVICE [DEVICE...] # labeled snapshot\n' "$0"
}
if [[ ${1:-} == --help ]]; then usage; exit 0; fi

discovery=false
if (( $# == 0 )); then discovery=true; fi
label=${1:-discovery}
if [[ ! $label =~ ^[a-zA-Z0-9_-]+$ ]]; then
    printf 'Expected a label before device arguments.\n' >&2
    usage >&2
    exit 2
fi
if (( $# == 1 )); then
    printf 'A labeled snapshot requires at least one device node.\n' >&2
    usage >&2
    exit 2
fi
if (( $# )); then shift; fi
for node in "$@"; do
    if [[ ! $node =~ ^/dev/(hidraw[0-9]+|input/event[0-9]+)$ ]]; then
        printf 'Not an input device path: %s\n' "$node" >&2
        exit 2
    fi
done

printf 'snapshot=%s utc=%s\n' "$label" "$(date -u +%FT%TZ)"
for field in sys_vendor product_name board_name; do
    file=/sys/class/dmi/id/$field
    if [[ -r $file ]]; then printf '%s=%s\n' "$field" "$(<"$file")"; fi
done
printf 'kernel=%s session=%s\n' "$(uname -r)" "${XDG_SESSION_TYPE:-unknown}"
if [[ -r /etc/os-release ]]; then
    sed -nE '/^(ID|VERSION_ID|BUILD_ID)=/p' /etc/os-release
fi
if ! command -v pacman >/dev/null; then
    printf 'Gamescope package version skipped: pacman unavailable\n'
elif ! pacman -Q gamescope 2>/dev/null && ! pacman -Q gamescope-plus 2>/dev/null; then
    printf 'Gamescope package version unknown: neither package is registered\n'
fi

printf '\nInput device inventory (check locally before choosing Deck nodes):\n'
for path in /sys/class/input/event* /sys/class/hidraw/hidraw*; do
    device=$path/device
    if [[ ! -d $device ]]; then continue; fi
    syspath=$(readlink -f -- "$device") || continue
    case $path in
        */event*)
            [[ -r $device/name && -r $device/capabilities/ev ]] || continue
            printf '/dev/input/%s name=%s events=%s path=%s\n' "${path##*/}" \
                "$(<"$device/name")" "$(<"$device/capabilities/ev")" "$syspath"
            ;;
        */hidraw*)
            [[ -r $device/uevent ]] || continue
            printf '/dev/%s path=%s\n' "${path##*/}" "$syspath"
            sed -nE '/^HID_(ID|NAME)=/p' "$device/uevent"
            ;;
    esac
done
if $discovery; then exit 0; fi

printf '\nRelevant processes and descendants (PID, PPID, command, no arguments):\n'
ps -ww -eo pid=,ppid=,comm= | awk '
{
    pid[NR] = $1
    parent[$1] = $2
    line[$1] = $0
    if ($3 ~ /^(aurora|steam|gamescope|Xwayland|inputplumber|kwin_|Xorg|reaper|pv-adverb|wineserver)/) {
        root[$1] = 1
    }
}
END {
    for (i = 1; i <= NR; i++) {
        for (p = pid[i]; p in parent && p != 0 && p != parent[p]; p = parent[p]) {
            if (root[p]) { print line[pid[i]]; break }
        }
    }
}'

if [[ -n ${HOME:-} && -r $HOME/.steam/steam.pid ]]; then
    steam_pid=$(<"$HOME/.steam/steam.pid")
    if [[ $steam_pid =~ ^[0-9]+$ ]]; then
        printf '\nSteam PID file points to PID %s: ' "$steam_pid"
        if [[ -r /proc/$steam_pid/comm ]]; then
            cat "/proc/$steam_pid/comm" || printf 'no longer visible\n'
        else
            printf 'process unavailable\n'
        fi
    fi
fi

printf '\nDevice holders (caller permissions can hide processes):\n'
for node in "$@"; do
    printf '%s:\n' "$node"
    if [[ ! -c $node ]]; then
        printf '  missing or no longer a character device; check the inventory\n'
        continue
    fi
    if command -v lsof >/dev/null; then
        # Query each node, because lsof can return partial results on a list.
        holders=$(lsof -wnP -Fpcn -- "$node") || true
        if [[ -n $holders ]]; then printf '%s\n' "$holders"; continue; fi
    fi
    found=false
    for entry in /proc/[0-9]*/fd; do
        process=${entry%/fd}
        for fd in "$entry"/*; do
            if [[ $fd -ef $node ]]; then
                name=$(cat "$process/comm" 2>/dev/null) || continue
                printf '  pid=%s command=%s (proc fallback)\n' "${process##*/}" "$name"
                found=true
                break
            fi
        done
    done
    if ! $found; then printf '  no holders visible\n'; fi
done

printf '\nX11 focus and Gamescope state on the selected display:\n'
if [[ -z ${DISPLAY:-} ]]; then
    printf 'Skipped: DISPLAY unset\n'
elif ! command -v xprop >/dev/null || ! command -v timeout >/dev/null; then
    printf 'Skipped: xprop or timeout unavailable\n'
else
    printf 'DISPLAY=%s\n' "$DISPLAY"
    if properties=$(timeout 4 xprop -root _NET_ACTIVE_WINDOW GAMESCOPE_FOCUSED_WINDOW \
        GAMESCOPE_FOCUSED_APP GAMESCOPECTRL_BASELAYER_APPID 2>/dev/null); then
        printf '%s\n' "$properties"
        active=$(printf '%s\n' "$properties" |
            sed -nE 's/^_NET_ACTIVE_WINDOW.*# (0x[[:xdigit:]]+)$/\1/p')
        if [[ -z $active || $active == 0x0 ]]; then
            active=$(printf '%s\n' "$properties" |
                sed -nE 's/^GAMESCOPE_FOCUSED_WINDOW[^=]*= (0x[[:xdigit:]]+|[0-9]+)$/\1/p')
        fi
        if [[ -n $active && $active != 0x0 && $active != 0 ]]; then
            printf 'Window %s identity (no title):\n' "$active"
            if identity=$(timeout 4 xprop -id "$active" WM_CLASS _NET_WM_PID STEAM_GAME 2>/dev/null); then
                printf '%s\n' "$identity"
                window_pid=$(printf '%s\n' "$identity" |
                    sed -nE 's/^_NET_WM_PID[^=]*= ([0-9]+)$/\1/p')
                if [[ -n $window_pid && ! -d /proc/$window_pid ]]; then
                    printf 'Window PID no longer visible; the identity may be stale.\n'
                fi
            else
                printf 'Window disappeared or belongs to another display.\n'
            fi
        else
            printf 'No active X11 window reported.\n'
        fi
    else
        printf 'Query unavailable on the selected display.\n'
    fi
fi
