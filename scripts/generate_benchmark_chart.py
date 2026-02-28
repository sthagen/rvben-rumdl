#!/usr/bin/env python3
"""
Generate benchmark comparison chart from hyperfine results.

Creates a transparent SVG chart that works in both light and dark modes,
following ruff's minimalistic design principles.
"""

import json
import re
import sys
from datetime import datetime
from pathlib import Path


def generate_chart():
    """Generate transparent SVG chart from benchmark results."""
    # Read results
    result_file = Path("benchmark/results/cold_start.json")
    if not result_file.exists():
        print(f"❌ Benchmark results not found: {result_file}")
        print("   Run scripts/benchmark_cold_start.py first")
        sys.exit(1)

    with open(result_file) as f:
        data = json.load(f)

    # Import matplotlib here to provide better error message
    try:
        import matplotlib.pyplot as plt
    except ImportError:
        print("❌ matplotlib not found")
        print("   This script uses uv to automatically install matplotlib")
        sys.exit(1)

    # Extract data
    results = data["results"]
    tools = [r["command"] for r in results]
    times = [r["mean"] * 1000 for r in results]  # Convert to milliseconds

    # Sort by time (fastest first)
    sorted_data = sorted(zip(tools, times), key=lambda x: x[1])
    tools, times = zip(*sorted_data)

    # Dynamic figure height: 0.5 inches per bar, minimum 2.5
    n_bars = len(tools)
    fig_height = max(2.5, 0.5 * n_bars + 0.5)

    # Create figure - transparent background
    fig, ax = plt.subplots(figsize=(10, fig_height))
    fig.patch.set_alpha(0.0)
    ax.patch.set_alpha(0.0)

    # Color scheme: vibrant green for rumdl, light gray for others
    colors = []
    for tool in tools:
        if tool == "rumdl":
            colors.append("#10b981")  # Vibrant emerald green
        else:
            colors.append("#e5e7eb")  # Very light gray

    # Create horizontal bars
    y_pos = range(len(tools))
    bars = ax.barh(y_pos, times, color=colors, height=0.6, edgecolor="none")

    # Set y-axis labels
    ax.set_yticks(y_pos)
    ax.set_yticklabels(tools, fontsize=11)

    # Make rumdl label stand out
    for tick, tool in zip(ax.get_yticklabels(), tools):
        if tool == "rumdl":
            tick.set_fontweight("bold")
            tick.set_fontsize(12)
            tick.set_color("#10b981")
        else:
            tick.set_color("#9ca3af")

    # Add value labels outside the bars
    for bar, time in zip(bars, times):
        width = bar.get_width()
        if time < 1000:
            label = f"{time:.0f}ms"
        else:
            label = f"{time / 1000:.1f}s"
        ax.text(
            width + (max(times) * 0.01),
            bar.get_y() + bar.get_height() / 2,
            label,
            ha="left",
            va="center",
            fontsize=10,
            color="#666666",
            fontweight="500",
        )

    # Subtle gridlines
    ax.grid(
        axis="x", alpha=0.2, linestyle="-", linewidth=0.5, color="#888888", zorder=0
    )
    ax.set_axisbelow(True)

    # Remove spines
    for spine in ax.spines.values():
        spine.set_visible(False)

    # X-axis: keep ticks subtle, no label (values on bars)
    ax.set_xlabel("")
    ax.tick_params(axis="x", labelsize=9, colors="#666666")

    # No title
    ax.set_title("")

    plt.tight_layout()

    # Save as SVG to assets/
    output_path = Path("assets/benchmark.svg")
    plt.savefig(
        output_path,
        bbox_inches="tight",
        facecolor="none",
        transparent=True,
        pad_inches=0.2,
        format="svg",
    )
    print(f"✅ Chart saved to {output_path}")

    # Also save to benchmark/results/ for reference
    intermediate_path = Path("benchmark/results/cold_start_comparison.svg")
    plt.savefig(
        intermediate_path,
        bbox_inches="tight",
        facecolor="none",
        transparent=True,
        pad_inches=0.2,
        format="svg",
    )
    print(f"✅ Intermediate chart saved to {intermediate_path}")


def update_comparison_doc(results_file):
    """Update dates and results table in docs/comparison.md from benchmark JSON."""
    doc_path = Path("docs/comparison.md")
    if not doc_path.exists():
        print("⚠️  docs/comparison.md not found, skipping doc update")
        return

    with open(results_file) as f:
        data = json.load(f)

    content = doc_path.read_text()
    date_str = datetime.now().strftime("%B %Y")

    # Update "Last verified: <month> <year>." on page header
    content, n1 = re.subn(
        r"Last verified: \w+ \d{4}\.",
        f"Last verified: {date_str}.",
        content,
    )
    if n1 == 0:
        print("⚠️  Could not find 'Last verified' date to update")

    # Update "Last run:\n> <month> <year>." in methodology blockquote
    content, n2 = re.subn(
        r"Last run:\n> \w+ \d{4}\.",
        f"Last run:\n> {date_str}.",
        content,
    )
    if n2 == 0:
        print("⚠️  Could not find 'Last run' date to update")

    # Rebuild the results table from JSON
    results = data["results"]
    # Category lookup (tools the script knows about)
    categories = {
        "rumdl": "Lint",
        "markdownlint-cli": "Lint",
        "markdownlint-cli2": "Lint",
        "remark-lint": "Lint",
        "pymarkdown": "Lint",
        "mado": "Lint",
        "mdformat": "Format",
        "Prettier": "Format",
    }

    sorted_results = sorted(results, key=lambda r: r["mean"])
    rumdl_mean = next(
        (r["mean"] for r in sorted_results if r["command"] == "rumdl"), None
    )

    # Build table rows
    rows = []
    for r in sorted_results:
        name = r["command"]
        mean_s = r["mean"]
        cat = categories.get(name, "Lint")

        if mean_s < 1:
            mean_str = f"{mean_s * 1000:.0f} ms"
        else:
            mean_str = f"{mean_s:.1f} s"

        if rumdl_mean and rumdl_mean > 0:
            ratio = mean_s / rumdl_mean
            if ratio < 0.1:
                vs_str = f"{ratio:.2f}x"
            else:
                vs_str = f"{ratio:.1f}x"
        else:
            vs_str = "-"

        bold_name = f"**{name}**"
        rows.append(f"| {bold_name:<23s} | {cat:<6s} | {mean_str:<6s} | {vs_str:<8s} |")

    # Find and replace the table between "| Tool" header and the next blank line
    table_header = (
        "| Tool                    | Type   | Mean   | vs rumdl |\n"
        "| ----------------------- | ------ | ------ | -------- |"
    )
    table_pattern = re.compile(
        r"\| Tool\s+\| Type\s+\| Mean\s+\| vs rumdl\s*\|\n"
        r"\|[\s-]+\|[\s-]+\|[\s-]+\|[\s-]+\|\n"
        r"(?:\|.*\|\n)*",
    )
    new_table = table_header + "\n" + "\n".join(rows) + "\n"
    if not table_pattern.search(content):
        print("⚠️  Could not find benchmark results table to update")
    content = table_pattern.sub(new_table, content)

    doc_path.write_text(content)
    print(f"✅ Updated dates and results in {doc_path}")


def main():
    """Main chart generation workflow."""
    project_root = Path(__file__).parent.parent
    import os

    os.chdir(project_root)

    print("📊 Generating benchmark comparison chart")
    print("=" * 50)

    generate_chart()
    update_comparison_doc(Path("benchmark/results/cold_start.json"))

    print("\n" + "=" * 50)
    print("✅ Chart generation complete!")
    print("\nThe chart is ready for use in docs/comparison.md")


if __name__ == "__main__":
    main()
