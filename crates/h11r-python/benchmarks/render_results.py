"""Render the public benchmark chart and table from one pyperf result."""

from __future__ import annotations

import argparse
from html import escape
from math import ceil
from pathlib import Path

import pyperf

SCENARIOS = (
    ("small_get", "Small GET"),
    ("fixed_body_1k", "Fixed body · 1 KiB"),
    ("fragmented_get_16b", "Fragmented GET · 16 B"),
    ("chunked_stream_64k", "Chunked stream · 64 KiB"),
    ("client_server_round_trip", "Client/server round trip"),
)


def load_results(path: Path) -> tuple[list[tuple[str, float, float]], str]:
    suite = pyperf.BenchmarkSuite.load(str(path))
    means = {benchmark.get_name(): benchmark.mean() for benchmark in suite}
    rows = []
    for key, label in SCENARIOS:
        try:
            h11r = means[f"scenario/{key}/h11r"]
            h11 = means[f"scenario/{key}/h11-0.16.0"]
        except KeyError as error:
            raise ValueError(f"missing benchmark: {error.args[0]}") from None
        rows.append((label, h11r, h11))

    metadata = suite.get_metadata()
    _, finished = suite.get_dates()
    versions = [
        f"h11r {metadata['h11r_version']}",
        f"h11 {metadata['h11_version']}",
        f"CPython {metadata['python_version']}",
    ]
    environment = [
        metadata.get("machine_name")
        or metadata.get("cpu_model_name")
        or f"{metadata.get('cpu_count', '?')} CPUs",
        metadata.get("platform"),
        finished.date().isoformat(),
    ]
    return rows, "\n".join(
        " · ".join(str(value) for value in line if value)
        for line in (versions, environment)
    )


def render_svg(rows: list[tuple[str, float, float]], details: str) -> str:
    width = 960
    plot_x, plot_width = 330, 560
    row_height = 76
    detail_lines = details.splitlines()
    height = 128 + row_height * len(rows) + 20 * len(detail_lines)
    speedups = [h11 / h11r for _, h11r, h11 in rows]
    tick_step = max(1, ceil(max(speedups) / 4))
    axis_max = tick_step * ceil(max(speedups) / tick_step)

    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">',
        '<title id="title">h11r and h11 Python benchmark</title>',
        '<desc id="desc">Relative throughput for five HTTP/1.1 scenarios. Higher is faster; h11 is the 1.00 baseline.</desc>',
        "<style>",
        "text{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif}",
        ".canvas{fill:#fbfbfd}.primary{fill:#1d1d1f}.secondary{fill:#6e6e73}"
        ".grid{stroke:#e5e5ea}",
        ".candidate{fill:#d45d3f}.reference{fill:#c7c7cc}",
        "</style>",
        f'<rect class="canvas" width="{width}" height="{height}"/>',
        '<text class="secondary" x="40" y="27" font-size="11" font-weight="500" '
        'letter-spacing="1.4">PYTHON · HTTP/1.1</text>',
        '<text class="primary" x="40" y="58" font-size="25" font-weight="500" '
        'letter-spacing="-0.4">Relative throughput by scenario</text>',
        '<text class="secondary" x="40" y="83" font-size="13">'
        "h11 0.16.0 = 1.00× · higher is faster</text>",
    ]

    for index in range(5):
        value = axis_max * index / 4
        x = plot_x + plot_width * index / 4
        svg.extend(
            [
                f'<line class="grid" x1="{x:.1f}" y1="108" x2="{x:.1f}" '
                f'y2="{112 + row_height * len(rows)}"/>',
                f'<text class="secondary" x="{x:.1f}" y="102" text-anchor="middle" '
                f'font-size="10">{value:.0f}×</text>',
            ]
        )

    for index, (label, h11r, h11) in enumerate(rows):
        speedup = h11 / h11r
        y = 124 + index * row_height
        h11_width = plot_width / axis_max
        h11r_width = plot_width * speedup / axis_max
        svg.extend(
            [
                f'<text class="primary" x="40" y="{y + 15}" font-size="13" '
                f'font-weight="500">{escape(label)}</text>',
                f'<text class="secondary" x="40" y="{y + 37}" font-size="11">'
                f"{h11r * 1e6:.2f} µs vs {h11 * 1e6:.2f} µs</text>",
                f'<text class="secondary" x="278" y="{y + 12}" text-anchor="end" '
                'font-size="10">h11r</text>',
                f'<rect class="candidate" x="{plot_x}" y="{y}" width="{h11r_width:.1f}" '
                'height="15" rx="7.5"/>',
                f'<text class="primary" x="{min(plot_x + h11r_width + 10, 920):.1f}" '
                f'y="{y + 12}" font-size="11" font-weight="500">{speedup:.1f}×</text>',
                f'<text class="secondary" x="278" y="{y + 35}" text-anchor="end" '
                'font-size="10">h11</text>',
                f'<rect class="reference" x="{plot_x}" y="{y + 23}" width="{h11_width:.1f}" '
                'height="10" rx="5"/>',
            ]
        )

    for index, line in enumerate(detail_lines):
        svg.append(
            f'<text class="secondary" x="40" y="{height - 30 + index * 17}" '
            f'font-size="10">{escape(line)}</text>'
        )
    svg.append("</svg>")
    return "\n".join(svg) + "\n"


def render_table(rows: list[tuple[str, float, float]], details: str) -> str:
    lines = [
        "| Scenario | h11r (µs/op) | h11 0.16.0 (µs/op) | Relative throughput |",
        "| --- | ---: | ---: | ---: |",
    ]
    for label, h11r, h11 in rows:
        lines.append(
            f"| {label} | {h11r * 1e6:.2f} | {h11 * 1e6:.2f} | {h11 / h11r:.1f}× |"
        )
    if details:
        lines.extend(("", f"_Environment: {details.replace(chr(10), ' · ')}_"))
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    parser.add_argument("--svg", required=True, type=Path)
    parser.add_argument("--table", type=Path)
    args = parser.parse_args()

    rows, details = load_results(args.results)
    args.svg.parent.mkdir(parents=True, exist_ok=True)
    args.svg.write_text(render_svg(rows, details), encoding="utf-8")
    table = render_table(rows, details)
    if args.table:
        args.table.parent.mkdir(parents=True, exist_ok=True)
        args.table.write_text(table, encoding="utf-8")
    else:
        print(table, end="")


if __name__ == "__main__":
    main()
