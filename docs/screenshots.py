#!/usr/bin/env python3
"""Makes the README's screenshots, docs/images/*.svg, from gclic itself.

It builds a demo repo (a made-up Python project) in target/screenshots, runs
the release build of gclic on it in a pseudo-terminal, presses keys, reads the
screen with the pyte terminal emulator, and draws it as SVG. Box-drawing lines
and the graph's beads are drawn as shapes, since browsers' fonts don't have
the bead characters that Ghostty and kitty draw.

Run it from the repo root, after any change to how gclic looks:

    python3 docs/screenshots.py

It installs pyte into target/screenshots the first time, and builds gclic.
"""

import fcntl
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent
WORK = ROOT / "target" / "screenshots"
IMAGES = ROOT / "docs" / "images"
GCLIC = ROOT / "target" / "release" / "gclic"
COLS, ROWS = 120, 34

# ---------------------------------------------------------------- setup


def load_pyte():
    packages = WORK / "py"
    if not (packages / "pyte").exists():
        subprocess.run(
            [sys.executable, "-m", "pip", "install", "-q", "--target", str(packages), "pyte"],
            check=True,
        )
    sys.path.insert(0, str(packages))
    import pyte

    return pyte


# git without the user's own settings, so the demo looks the same everywhere.
GIT_ENV = {
    **os.environ,
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_NOSYSTEM": "1",
    "TERM": "xterm-256color",
    # As Ghostty, so gclic draws the graph's beads joined up, which the
    # screenshots then draw as shapes.
    "TERM_PROGRAM": "ghostty",
}


class Demo:
    """A git repo whose commits get fixed authors and dates."""

    def __init__(self, path):
        self.path = path
        self.clock = 1_767_000_000  # late December 2025

    def git(self, *args, author="Ana Ruiz"):
        self.clock += 3 * 3600
        email = author.split()[0].lower() + "@example.com"
        date = f"{self.clock} +0100"
        env = {
            **GIT_ENV,
            "GIT_AUTHOR_NAME": author,
            "GIT_AUTHOR_EMAIL": email,
            "GIT_COMMITTER_NAME": author,
            "GIT_COMMITTER_EMAIL": email,
            "GIT_AUTHOR_DATE": date,
            "GIT_COMMITTER_DATE": date,
        }
        subprocess.run(["git", *args], cwd=self.path, env=env, check=True, capture_output=True)

    def write(self, name, text):
        file = self.path / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text.lstrip("\n"))

    def commit(self, message, author="Ana Ruiz"):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message, author=author)


MODELS = '''
from dataclasses import dataclass


@dataclass
class Forecast:
    city: str
    temperature: float
    summary: str

    def describe(self) -> str:
        return f"{self.city}: {self.temperature:.0f}°, {self.summary}"
'''

API = '''
import json
from urllib.request import urlopen

from .models import Forecast

BASE_URL = "https://api.example.com/v1"


def fetch_forecast(city: str) -> Forecast:
    """Fetches today's forecast for a city."""
    with urlopen(f"{BASE_URL}/forecast?city={city}") as response:
        data = json.load(response)
    return Forecast(city, data["temp"], data["summary"])


def fetch_alerts(city: str) -> list[str]:
    """Fetches the weather alerts in force for a city."""
    with urlopen(f"{BASE_URL}/alerts?city={city}") as response:
        data = json.load(response)
    return [alert["title"] for alert in data["alerts"]]


def fetch_air_quality(city: str) -> int:
    """Fetches a city's air quality index."""
    with urlopen(f"{BASE_URL}/air?city={city}") as response:
        data = json.load(response)
    return data["aqi"]


def fetch_uv_index(city: str) -> float:
    """Fetches today's highest UV index for a city."""
    with urlopen(f"{BASE_URL}/uv?city={city}") as response:
        data = json.load(response)
    return data["uv"]
'''

CLI = '''
import sys

from .api import fetch_forecast


def main() -> None:
    city = sys.argv[1] if len(sys.argv) > 1 else "Madrid"
    print(fetch_forecast(city).describe())
'''


def build_demo():
    if WORK.exists():
        shutil.rmtree(WORK / "repos", ignore_errors=True)
    repos = WORK / "repos"
    repos.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(repos / "origin.git")], env=GIT_ENV, check=True)

    demo = Demo(repos / "skycast")
    demo.path.mkdir()
    demo.git("init", "-q", "-b", "main")
    demo.git("remote", "add", "origin", str(repos / "origin.git"))
    demo.write("README.md", "# skycast\n\nWeather forecasts in your terminal.\n")
    demo.write("pyproject.toml", '[project]\nname = "skycast"\nversion = "0.1.0"\n')
    demo.commit("Initial project layout")
    demo.write("skycast/models.py", MODELS)
    demo.commit("Add forecast models", author="Ben Okafor")
    demo.write("skycast/api.py", API)
    demo.write("skycast/cli.py", CLI)
    demo.commit("Fetch forecasts from the API")

    # A branch merged back, so the graph has a fork and a join.
    demo.git("switch", "-q", "-c", "units")
    demo.write("skycast/models.py", MODELS.replace("    summary: str\n", '    summary: str\n    units: str = "C"\n'))
    demo.commit("Support Fahrenheit", author="Kai Tanaka")
    demo.write("skycast/models.py", MODELS.replace("    summary: str\n", '    summary: str\n    units: str = "C"\n').replace(":.0f}°,", ':.0f}°{self.units},'))
    demo.commit("Show units in descriptions", author="Kai Tanaka")
    demo.git("switch", "-q", "main")
    demo.write("skycast/api.py", API.replace("import json\n", "import json\nfrom functools import lru_cache\n").replace("def fetch_forecast", "@lru_cache(maxsize=64)\ndef fetch_forecast"))
    demo.commit("Cache API responses")
    demo.git("merge", "-q", "--no-ff", "-m", "Merge branch 'units'", "units")
    demo.write("tests/test_api.py", 'from skycast.models import Forecast\n\n\ndef test_describe():\n    forecast = Forecast("Oslo", -3.2, "snow")\n    assert forecast.describe().startswith("Oslo")\n')
    demo.commit("Test forecast descriptions", author="Ben Okafor")
    demo.git("push", "-q", "-u", "origin", "main")

    # The checked-out branch: pushed once, then one more commit not yet pushed.
    demo.git("switch", "-q", "-c", "hourly")
    hourly_api = API + '''

def fetch_hourly(city: str, hours: int = 12) -> list[Forecast]:
    """Fetches the next hours' forecasts for a city."""
    with urlopen(f"{BASE_URL}/hourly?city={city}&hours={hours}") as response:
        data = json.load(response)
    return [Forecast(city, hour["temp"], hour["summary"]) for hour in data["hours"]]
'''
    demo.write("skycast/api.py", hourly_api)
    demo.commit("Add hourly forecasts", author="Kai Tanaka")
    demo.git("push", "-q", "-u", "origin", "hourly")
    demo.write("skycast/api.py", hourly_api.replace('for hour in data["hours"]]', 'for hour in data.get("hours", [])]'))
    demo.commit("Handle forecasts without hours", author="Kai Tanaka")

    # A branch that only exists on the remote.
    demo.git("push", "-q", "origin", "main:docs")

    # Work in progress: one change staged, one not, and a new file.
    demo.write("skycast/cli.py", CLI.replace('"Madrid"', '"Lisbon"'))
    demo.git("add", "skycast/cli.py")
    demo.write("skycast/api.py", hourly_api.replace('for hour in data["hours"]]', 'for hour in data.get("hours", [])]').replace("hours: int = 12", "hours: int = 24").replace("next hours'", "next day's"))
    demo.write("NOTES.md", "- Ask about rate limits\n")

    # A second clone, stopped on a merge conflict.
    conflict = Demo(repos / "skycast-conflict")
    subprocess.run(["git", "clone", "-q", str(repos / "origin.git"), str(conflict.path)], env=GIT_ENV, check=True)
    conflict.clock = demo.clock
    conflict.git("switch", "-q", "-c", "rounding")
    conflict.write("skycast/models.py", (repos / "skycast" / "skycast" / "models.py").read_text().replace(":.0f}", ":.1f}").replace("return f", "# One decimal, as stations report.\n        return f"))
    conflict.commit("Show one decimal", author="Ben Okafor")
    conflict.git("switch", "-q", "main")
    conflict.write("skycast/models.py", (repos / "skycast" / "skycast" / "models.py").read_text().replace("°{self.units}", " °{self.units}"))
    conflict.commit("Space before the degree sign")
    subprocess.run(["git", "merge", "-q", "rounding"], cwd=conflict.path, env=GIT_ENV, capture_output=True)
    return demo.path, conflict.path


# ------------------------------------------------------------ capturing


def capture(pyte, repo, keys, settle=1.2):
    """Runs gclic in `repo`, presses `keys`, and returns the screen."""
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(repo)
        # The demo's folder as home, so the tab bar shows ~/skycast rather
        # than wherever this checkout happens to be.
        # The same editor in every screenshot, whoever makes them.
        env = {**GIT_ENV, "HOME": str(Path(repo).parent), "EDITOR": "nvim"}
        for name in ("GCLIC_EDITOR", "VISUAL", "NVIM"):
            env.pop(name, None)
        os.execve(str(GCLIC), [str(GCLIC)], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)

    def wait(seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.01)
            if ready:
                stream.feed(os.read(fd, 1 << 20))

    wait(1.5)
    for key in keys:
        os.write(fd, key.encode())
        wait(0.35)
    wait(settle)  # for colours from the background worker
    # A popup would take ctrl-c as a key to close it, so gclic is stopped.
    os.kill(pid, 9)
    os.waitpid(pid, 0)
    os.close(fd)
    return screen


# ------------------------------------------------------------- drawing

CELL_W, CELL_H, FONT = 8.2, 17, 13.6


def terminal_colours():
    """The background, text colour and 16 basic colours to draw with: the
    ones Ghostty uses, read from it if it's installed, or else its defaults.
    gclic's own exact colours, such as syntax highlighting, don't need them."""
    background, foreground = "#282c34", "#ffffff"
    palette = [
        "#1d1f21", "#cc6666", "#b5bd68", "#f0c674", "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
        "#666666", "#d54e53", "#b9ca4a", "#e7c547", "#7aa6da", "#c397d8", "#70c0b1", "#eaeaea",
    ]
    try:
        config = subprocess.run(["ghostty", "+show-config", "--default"], capture_output=True, text=True, timeout=10).stdout
    except (OSError, subprocess.TimeoutExpired):
        config = ""
    for line in config.splitlines():
        key, _, value = line.partition(" = ")
        if key == "background" and value:
            background = value
        elif key == "foreground" and value:
            foreground = value
        elif key == "palette" and "=" in value:
            index, colour = value.split("=", 1)
            if int(index) < 16:
                palette[int(index)] = colour
    return background, foreground, palette


BG, FG, PALETTE = terminal_colours()
# pyte names the 16 basic colours, in palette order. Yellow is "brown".
NAMES = ["black", "red", "green", "brown", "blue", "magenta", "cyan", "white"]
NAMED = {name: PALETTE[i] for i, name in enumerate(NAMES)}
NAMED.update({"bright" + name: PALETTE[i + 8] for i, name in enumerate(NAMES)})
NAMED["yellow"], NAMED["brightyellow"] = NAMED["brown"], NAMED["brightbrown"]


# pyte also reports the 16 basic colours as xterm's values for them, in
# palette order, rather than by name.
XTERM = ["000000", "cd0000", "00cd00", "cdcd00", "0000ee", "cd00cd", "00cdcd", "e5e5e5",
         "7f7f7f", "ff0000", "00ff00", "ffff00", "5c5cff", "ff00ff", "00ffff", "ffffff"]


def color(value, default):
    if value == "default":
        return default
    if value.lower() in XTERM:
        return PALETTE[XTERM.index(value.lower())]
    return NAMED.get(value, f"#{value}")


# Which edges of its cell each line character reaches: up, down, left, right.
LINES = {
    "│": "ud", "─": "lr", "├": "udr", "┤": "udl", "┬": "dlr", "┴": "ulr", "┼": "udlr",
    "╭": "dr", "╮": "dl", "╰": "ur", "╯": "ul",
}
# The branch-drawing beads, filled ones first in each pair, by the edges their
# lines reach. Same order as bead_char in src/graph.rs.
BEAD_EDGES = ["", "r", "l", "lr", "d", "u", "ud", "dr", "dl", "ur", "ul", "udr", "udl", "dlr", "ulr", "udlr"]
BEADS = {chr(0xF5EE + 2 * i + hollow): (edges, hollow == 1) for i, edges in enumerate(BEAD_EDGES) for hollow in (0, 1)}


def line_shape(x, y, edges, stroke, rounded):
    cx, cy = x + CELL_W / 2, y + CELL_H / 2
    ends = {"u": (cx, y), "d": (cx, y + CELL_H), "l": (x, cy), "r": (x + CELL_W, cy)}
    if rounded and len(edges) == 2:
        (ax, ay), (bx, by) = ends[edges[0]], ends[edges[1]]
        return f'<path d="M{ax:.1f},{ay:.1f} Q{cx:.1f},{cy:.1f} {bx:.1f},{by:.1f}" fill="none" stroke="{stroke}" stroke-width="1.2"/>'
    return "".join(
        f'<line x1="{cx:.1f}" y1="{cy:.1f}" x2="{ex:.1f}" y2="{ey:.1f}" stroke="{stroke}" stroke-width="1.2"/>'
        for edge, (ex, ey) in ends.items()
        if edge in edges
    )


def bead_shape(x, y, edges, hollow, stroke, background):
    cx, cy, r = x + CELL_W / 2, y + CELL_H / 2, CELL_W * 0.4
    shape = line_shape(x, y, edges, stroke, rounded=False)
    fill = background if hollow else stroke
    return shape + f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="{r:.1f}" fill="{fill}" stroke="{stroke}" stroke-width="1.2"/>'


def to_svg(screen):
    width, height = COLS * CELL_W, ROWS * CELL_H
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}" viewBox="0 0 {width:.0f} {height:.0f}">',
        f'<rect width="100%" height="100%" rx="6" fill="{BG}"/>',
        '<g font-family="SFMono-Regular, Menlo, Consolas, \'Liberation Mono\', monospace" '
        f'font-size="{FONT}" xml:space="preserve">',
    ]
    for row in range(ROWS):
        line = screen.buffer[row]
        y = row * CELL_H
        col = 0
        while col < COLS:
            cell = line[col]
            background = color(cell.bg, BG)
            # A run of cells sharing a background and a text style.
            end = col
            while end < COLS and color(line[end].bg, BG) == background and (line[end].fg, line[end].bold) == (cell.fg, cell.bold):
                end += 1
            x = col * CELL_W
            if background != BG:
                parts.append(f'<rect x="{x:.1f}" y="{y}" width="{(end - col) * CELL_W:.1f}" height="{CELL_H}" fill="{background}" shape-rendering="crispEdges"/>')
            stroke = color(cell.fg, FG)
            text = ""
            text_start = col
            for c in range(col, end):
                char = line[c].data
                cx = c * CELL_W
                if char in LINES or char in BEADS:
                    if text.strip():
                        parts.append(text_element(text, text_start, y, stroke, cell.bold))
                    text, text_start = "", c + 1
                    if char in LINES:
                        parts.append(line_shape(cx, y, LINES[char], stroke, rounded=char in "╭╮╰╯"))
                    else:
                        edges, hollow = BEADS[char]
                        parts.append(bead_shape(cx, y, edges, hollow, stroke, background))
                else:
                    text += char
            if text.strip():
                parts.append(text_element(text, text_start, y, stroke, cell.bold))
            col = end
    parts.append("</g></svg>")
    return "\n".join(parts)


def text_element(text, col, y, fill, bold):
    # One element per word, each at its own cell. Renderers may collapse runs
    # of spaces, or drop them at the ends, and then stretch the rest to fill
    # the length, which would distort the glyphs.
    weight = ' font-weight="bold"' if bold else ""
    return "\n".join(
        f'<text x="{(col + word.start()) * CELL_W:.1f}" y="{y + CELL_H * 0.74:.1f}" fill="{fill}"{weight} '
        f'textLength="{len(word.group()) * CELL_W:.1f}" lengthAdjust="spacingAndGlyphs">{escape(word.group())}</text>'
        for word in re.finditer(r"\S+", text)
    )


# ---------------------------------------------------------------- shots

ENTER, ESC, TAB = "\r", "\x1b", "\t"


def main():
    pyte = load_pyte()
    subprocess.run(["cargo", "build", "--release", "-q"], cwd=ROOT, check=True)
    repo, conflict = build_demo()
    shots = {
        # Changes: the unstaged file's diff, next to the lists.
        "staging": (repo, ["j", ENTER]),
        # Graph: the top commit opened, with its details on the right.
        "graph": (repo, ["l", ENTER, "i"]),
        # Branches: the list, with main's remotes opened.
        "branches": (repo, ["h", "j", " "]),
        # A commit's diff, moved into, with the active line a few rows down.
        "diff": (repo, ["l", ENTER, "j", ENTER, ENTER, "j", "j", "j", "j", "j", "j"]),
        # Compare: main against hourly, with a file's diff.
        "compare": (repo, [TAB, "c", "i", *"main", ENTER, "i", *"hourly", ENTER, ENTER]),
        # A merge stopped on a conflict.
        "conflict": (conflict, [ENTER]),
        # The help, opened from the graph.
        "help": (repo, ["l", "?"]),
    }
    IMAGES.mkdir(parents=True, exist_ok=True)
    for name, (path, keys) in shots.items():
        screen = capture(pyte, path, keys)
        (IMAGES / f"{name}.svg").write_text(to_svg(screen))
        print(f"docs/images/{name}.svg")


if __name__ == "__main__":
    main()
