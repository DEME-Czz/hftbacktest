#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  scripts/collect_binance_futures_data.sh [--output DIR] SYMBOL [SYMBOL ...]

Examples:
  scripts/collect_binance_futures_data.sh DOGEUSDT
  scripts/collect_binance_futures_data.sh --output runtime/market-data DOGEUSDT ETHUSDT

This script only collects public Binance USD-M Futures market data. It does not
read API credentials, connect to an account, or submit/cancel orders.

Press Ctrl-C once to stop cleanly and finish the current gzip files.
EOF
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "${script_dir}/.." && pwd)"
output_root="${repo_dir}/runtime/market-data"
symbols=()

while (($# > 0)); do
    case "$1" in
        --output)
            if (($# < 2)); then
                echo "error: --output requires a directory" >&2
                usage >&2
                exit 2
            fi
            output_root="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            while (($# > 0)); do
                symbols+=("$1")
                shift
            done
            ;;
        -*)
            echo "error: unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            symbols+=("$1")
            shift
            ;;
    esac
done

if ((${#symbols[@]} == 0)); then
    echo "error: provide at least one Binance Futures symbol" >&2
    usage >&2
    exit 2
fi

for symbol in "${symbols[@]}"; do
    if [[ ! "$symbol" =~ ^[A-Za-z0-9]+$ ]]; then
        echo "error: invalid symbol '$symbol'; use names such as DOGEUSDT" >&2
        exit 2
    fi
done

if [[ "$output_root" != /* ]]; then
    output_root="${repo_dir}/${output_root}"
fi

session_id="$(date -u +%Y%m%dT%H%M%SZ)"
session_dir="${output_root}/raw/${session_id}"
mkdir -p "$session_dir"

collector_bin="${repo_dir}/target/release/collector"
echo "Checking the release collector build..."
cargo build --quiet --release --package collector --manifest-path "${repo_dir}/Cargo.toml"

echo "Public market-data collection only; no API key and no order access."
echo "Exchange: Binance USD-M Futures"
echo "Symbols: ${symbols[*]}"
echo "Output: ${session_dir}"
echo "Stop: Ctrl-C"

cd "$repo_dir"
RUST_LOG="${RUST_LOG:-info}" exec "$collector_bin" \
    "$session_dir" \
    binancefutures \
    "${symbols[@]}"
