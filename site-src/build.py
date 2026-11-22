#!/usr/bin/env python3
"""Build the website into docs/ (GitHub Pages: Settings > Pages > Deploy from a branch > main /docs).

    python3 site-src/build.py

Pages live in site-src/pages/*.html as body fragments. Benchmark numbers come from
site-src/data/*.json, produced by `mide-bench --json` and the app's MIDE_PERF_JSON.
No dependencies beyond Python 3.
"""
import html, json, pathlib, re, shutil, subprocess, sys

SRC = pathlib.Path(__file__).resolve().parent
ROOT = SRC.parent
OUT = ROOT / "docs"
CFG = json.loads((SRC / "config.json").read_text())

NAV = [("install", "Install"), ("guide", "Guide"), ("terminal", "Terminal"), ("git", "Git"),
       ("configuration", "Configuration"), ("performance", "Performance"),
       ("architecture", "Architecture"), ("roadmap", "Roadmap")]


# ---- benchmark rendering ---------------------------------------------------------------------

def fmt_ms(ms):
    if ms < 0.001:
        return "under 1 µs"
    if ms >= 1000:
        return f"{ms / 1000:.2f} s"
    if ms >= 10:
        return f"{ms:.0f} ms"
    if ms >= 1:
        return f"{ms:.1f} ms"
    return f"{ms * 1000:.0f} µs"


def load(name):
    return json.loads((SRC / "data" / name).read_text())


BENCH = load("bench.json")
APP = load("app-perf.json")
RESULTS = {r["name"]: r for r in BENCH["results"]}


def result(prefix):
    for name, r in RESULTS.items():
        if name.startswith(prefix):
            return r
    raise KeyError(f"no benchmark starting with {prefix!r}")


def bignum(value, label, note=""):
    n = f'<span class="note">{html.escape(note)}</span>' if note else ""
    return f'<div class="bignum"><b>{html.escape(value)}</b><span>{html.escape(label)}</span>{n}</div>'


def hero_figures():
    typing = APP["typing"]["frame_p50_ms"]
    open_ms = result("core: open file")["median_ms"]
    idle = APP["idle_cpu_percent"]
    figs = [
        (f"{typing:.1f} ms", "of work per keystroke"),
        (fmt_ms(open_ms), "to open 1,000,000 lines"),
        (f"{idle:.1f}%", "CPU when idle"),
    ]
    return "".join(f'<div class="figure"><b>{html.escape(v)}</b><span>{html.escape(l)}</span></div>' for v, l in figs)


def bench_cards():
    """The home page's four cards (the hero already shows typing, opening and idle)."""
    term = next(r for n, r in RESULTS.items() if n.startswith("terminal: take in"))
    cards = [
        bignum(fmt_ms(result("syntax: cold jump")["median_ms"]), "to jump to line 100,000", "with syntax colors"),
        bignum(f'{APP["scrolling"]["frame_p50_ms"]:.1f} ms', "of work per frame while scrolling", "median, in the real app"),
        bignum(fmt_ms(result("fuzzy: ⌘P over 100k paths, typical")["median_ms"]), "to search 100,000 file names", "for each keystroke"),
        bignum(fmt_ms(term["median_ms"]), "to take in 8 MB of terminal output", "from a real pty"),
    ]
    return '<div class="bignums">' + "".join(cards) + "</div>"


def headline_cards():
    cards = [
        bignum(f'{APP["typing"]["frame_p50_ms"]:.1f} ms', "of work for each keystroke", "median, in the real app"),
        bignum(fmt_ms(result("core: open file")["median_ms"]), "to open a 1,000,000 line file", f'{BENCH["meta"]["fixture_mb"]:.0f} MB'),
        bignum(fmt_ms(result("syntax: cold jump")["median_ms"]), "to jump to line 100,000", "with syntax colors"),
        bignum(fmt_ms(result("fuzzy: ⌘P over 100k paths, typical")["median_ms"]), "to search 100,000 file names", "for each keystroke"),
    ]
    return '<div class="bignums">' + "".join(cards) + "</div>"


def bench_table():
    groups = {}
    for r in BENCH["results"]:
        groups.setdefault(r["name"].split(":")[0], []).append(r)
    titles = {"core": "Text buffer", "search": "Find and replace", "syntax": "Syntax highlighting",
              "fuzzy": "Fuzzy file search", "files": "Files", "terminal": "Terminal", "git": "Git"}
    rows = []
    for g, rs in groups.items():
        rows.append(f'<tr class="group"><th colspan="5" scope="colgroup">{titles.get(g, g)}</th></tr>')
        for r in rs:
            pct = min(100.0, 100.0 * r["median_ms"] / r["budget_ms"]) if r["budget_ms"] else 0
            over = " over" if r["median_ms"] > r["budget_ms"] else ""
            label = r["name"].split(": ", 1)[1]
            rows.append(
                f'<tr><td>{html.escape(label)}</td><td class="num">{fmt_ms(r["median_ms"])}</td>'
                f'<td class="num">{fmt_ms(r["p95_ms"])}</td><td class="num">{fmt_ms(r["budget_ms"])}</td>'
                f'<td><span class="bar{over}" role="img" aria-label="{pct:.0f} percent of the budget"><i style="--pct:{pct:.1f}"></i></span></td></tr>')
    head = ('<thead><tr><th scope="col">Benchmark</th><th scope="col" class="num">Median</th><th scope="col" class="num">p95</th>'
            '<th scope="col" class="num">Budget</th><th scope="col">Share of budget used</th></tr></thead>')
    return f'<div class="table-wrap"><table>{head}<tbody>{"".join(rows)}</tbody></table></div>'


def app_table():
    a = APP
    rows = [
        ("Window created", f'{a["startup"]["window_created_ms"]:.0f} ms', "GPUI and Metal start-up, before mide's own code runs"),
        ("First editor paint", f'{a["startup"]["first_paint_ms"]:.0f} ms', "folder opened, syntax loaded, first window drawn"),
        ("Typing: frame work", f'{a["typing"]["frame_p50_ms"]:.1f} ms median, {a["typing"]["frame_p95_ms"]:.1f} ms p95',
         "layout and paint for one keystroke; the 16 ms frame budget is the limit"),
        ("Typing: input to paint", f'{a["typing"]["input_p50_ms"]:.1f} ms median, {a["typing"]["input_p95_ms"]:.1f} ms p95',
         "includes waiting for the next screen refresh (about 8 ms on average at 60 Hz)"),
        ("Scrolling, 3 lines per frame", f'{a["scrolling"]["frame_p50_ms"]:.1f} ms median, {a["scrolling"]["frame_p95_ms"]:.1f} ms p95', "frame work while scrolling"),
        ("Jump to the end of the file", f'{a["jump"]["input_ms"]:.0f} ms', "one frame, from keypress to painted and colored"),
        ("Idle CPU", f'{a["idle_cpu_percent"]:.1f}%', "editor and one terminal session open"),
        ("Memory", f'{a["memory_mb"]:.0f} MB', "same session"),
    ]
    body = "".join(f"<tr><td>{html.escape(k)}</td><td class='num'>{html.escape(v)}</td><td>{html.escape(n)}</td></tr>" for k, v, n in rows)
    return ('<div class="table-wrap"><table><thead><tr><th scope="col">Measure</th><th scope="col" class="num">Result</th>'
            f'<th scope="col">What it is</th></tr></thead><tbody>{body}</tbody></table></div>')


def pkg_size():
    pkgs = sorted((ROOT / "dist").glob("mide-*.pkg"))
    return f"{pkgs[0].stat().st_size / 1e6:.1f} MB" if pkgs else "about 6 MB"


SUBS = {
    "{{pkg_size}}": pkg_size(),
    "{{repo}}": CFG["repo"],
    "{{version}}": CFG["version"],
    "{{hero_figures}}": hero_figures(),
    "{{bench_cards}}": bench_cards(),
    "{{headline_cards}}": headline_cards(),
    "{{bench_table}}": bench_table(),
    "{{app_table}}": app_table(),
    "{{machine}}": html.escape(BENCH["meta"]["machine"]),
    "{{bench_date}}": html.escape(BENCH["meta"]["date"]),
    "{{fixture_lines}}": f'{BENCH["meta"]["fixture_lines"]:,}',
    "{{fixture_mb}}": f'{BENCH["meta"]["fixture_mb"]:.0f}',
    "{{count_benchmarks}}": str(len(BENCH["results"])),
}


# ---- pages ---------------------------------------------------------------------------------------

def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", re.sub(r"<[^>]+>", "", text).lower()).strip("-")


def add_ids_and_toc(body):
    heads = []

    def sub(m):
        inner = m.group(1)
        ident = slug(inner)
        heads.append((ident, re.sub(r"<[^>]+>", "", inner)))
        return f'<h2 id="{ident}">{inner}</h2>'

    body = re.sub(r"<h2>(.*?)</h2>", sub, body, flags=re.S)
    toc = ""
    if len(heads) >= 3:
        items = "".join(f'<li><a href="#{i}">{html.escape(t)}</a></li>' for i, t in heads)
        toc = f'<aside class="toc" aria-label="On this page"><p class="toc-title">On this page</p><ul>{items}</ul></aside>'
    return body, toc


def build_page(path):
    raw = path.read_text()
    meta = dict(re.findall(r"<!--(\w+):\s*(.*?)\s*-->", raw))
    body = re.sub(r"<!--\w+:.*?-->\s*", "", raw)
    for k, v in SUBS.items():
        body = body.replace(k, v)
    layout = meta.get("layout", "doc")
    if layout == "doc":
        body, toc = add_ids_and_toc(body)
        body = f'<div class="wrap page">{toc}<article class="prose">{body}</article></div>'
    key = path.stem
    nav = "\n".join(f'      <a href="{k}.html"{" aria-current=\"page\"" if k == key else ""}>{label}</a>' for k, label in NAV)
    page = (SRC / "template.html").read_text()
    for k, v in {"{{title}}": html.escape(meta["title"]), "{{desc}}": html.escape(meta["desc"]), "{{nav}}": nav, "{{body}}": body}.items():
        page = page.replace(k, v)
    for k, v in SUBS.items():
        page = page.replace(k, v)
    left = re.findall(r"\{\{\w+\}\}", page)
    if left:
        sys.exit(f"{key}: unreplaced placeholders {left}")
    (OUT / f"{key}.html").write_text(page)
    return key


def main():
    OUT.mkdir(exist_ok=True)
    for old in OUT.glob("*.html"):
        old.unlink()
    (OUT / "assets").mkdir(exist_ok=True)
    for f in (SRC / "assets").iterdir():
        shutil.copy(f, OUT / "assets" / f.name)
    icon = ROOT / "assets" / "macos" / "mide-1024.png"
    for size, name in [(32, "favicon-32.png"), (64, "icon-64.png"), (128, "icon-128.png"), (180, "icon-180.png")]:
        subprocess.run(["sips", "-z", str(size), str(size), str(icon), "--out", str(OUT / "assets" / name)],
                       check=True, stdout=subprocess.DEVNULL)
    (OUT / ".nojekyll").write_text("")
    pages = [build_page(p) for p in sorted((SRC / "pages").glob("*.html"))]
    check_links(pages)
    if CFG["repo"].startswith("OWNER/"):
        print('NOTE: site-src/config.json still has repo "OWNER/mide"; set it to your GitHub "user/repo" and rebuild.')
    print(f"built {len(pages)} pages into {OUT.relative_to(ROOT)}/")


def check_links(pages):
    bad = []
    ids = {p: set(re.findall(r'id="([^"]+)"', (OUT / f"{p}.html").read_text())) for p in pages}
    for p in pages:
        text = (OUT / f"{p}.html").read_text()
        for href in re.findall(r'href="([^"]+)"', text):
            if href.startswith(("http", "mailto:", "#")) and not href.startswith("#"):
                continue
            if href.startswith("#"):
                if href[1:] not in ids[p]:
                    bad.append(f"{p}: missing anchor {href}")
                continue
            target, _, frag = href.partition("#")
            if target.endswith(".html"):
                if target[:-5] not in pages:
                    bad.append(f"{p}: broken link {href}")
                elif frag and frag not in ids[target[:-5]]:
                    bad.append(f"{p}: missing anchor {href}")
            elif target.startswith("assets/") and not (OUT / target).exists():
                bad.append(f"{p}: missing file {href}")
        for src in re.findall(r'src="([^"]+)"', text):
            if src.startswith("assets/") and not (OUT / src).exists():
                bad.append(f"{p}: missing file {src}")
    if bad:
        print("\n".join(bad))
        sys.exit(1)


main()
