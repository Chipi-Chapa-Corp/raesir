#!/usr/bin/env python3
"""Fully mocked Qt Quick launcher. No accounts, scans, network or real actions."""
import colorsys
import os
import sys
from functools import lru_cache
from pathlib import Path
from xml.sax.saxutils import escape

from PySide6.QtCore import QObject, Property, Signal, Slot, QUrl, QRectF, Qt
from PySide6.QtGui import QGuiApplication, QImage, QPainter, QPainterPath, QColor
from PySide6.QtQml import QQmlApplicationEngine
from PySide6.QtQuick import QQuickImageProvider
from PySide6.QtSvg import QSvgRenderer

STORES = ["steam", "epic", "ea", "ubisoft", "xbox"]
TITLES = ["Among Us", "Ancestors: The Humankind Odyssey", "ARK: Survival Evolved",
          "Baba Is You", "Balatro", "Beat Saber", "Big Walk", "Bloons TD 6",
          "Chained Together", "Content Warning", "Core Keeper", "Counter-Strike 2",
          "Crab Game", "Dead by Daylight", "Deadlock", "Deep Rock Galactic",
          "Despotism 3k", "Don't Starve Together", "DOOM Eternal", "Dota 2",
          "Dying Light", "Fall Guys", "Geometry Dash", "Grounded 2", "Helltaker",
          "Hollow Knight", "Minecraft", "Noita", "PEAK", "Portal 2", "R.E.P.O.",
          "SpiderHeck", "Stardew Valley", "Terraria", "Timberborn", "Valheim"]

def raster(svg, width=240, height=320):
    image = QImage(width, height, QImage.Format.Format_ARGB32_Premultiplied)
    image.fill(Qt.GlobalColor.transparent)
    painter = QPainter(image)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    if width == 240:
        clip = QPainterPath(); clip.addRoundedRect(QRectF(0, 0, width, height), 18, 18)
        painter.setClipPath(clip)
    QSvgRenderer(svg.encode()).render(painter)
    painter.end()
    return image


@lru_cache(maxsize=96)
def poster(index, title, installed):
    colors = [colorsys.hsv_to_rgb((index * .137 + offset) % 1, .7, .8) for offset in [0, .14]]
    if not installed:
        colors = [(sum(rgb) / 3 * .55,) * 3 for rgb in colors]
    a, b = ["#" + "".join(f"{int(v * 255):02x}" for v in rgb) for rgb in colors]
    words = title.split(" · ")[0].upper().split()
    lines, line = [], ""
    for word in words:
        if len(line + word) > 15 and line:
            lines.append(line); line = ""
        line += (" " if line else "") + word
    lines.append(line)
    text = "".join(f'<text x="120" y="{225 + n * 23}" text-anchor="middle" fill="white" font-family="sans-serif" font-size="20" font-weight="bold">{escape(s)}</text>' for n, s in enumerate(lines[:3]))
    return raster(f'''<svg xmlns="http://www.w3.org/2000/svg" width="240" height="320" viewBox="0 0 240 320">
      <defs><linearGradient id="g" x2=".8" y2="1"><stop stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient>
      <linearGradient id="s" x2="0" y2="1"><stop stop-color="#000" stop-opacity=".1"/><stop offset="1" stop-color="#000" stop-opacity=".85"/></linearGradient></defs>
      <rect width="240" height="320" fill="url(#g)"/><circle cx="{55 + index % 130}" cy="105" r="80" fill="none" stroke="#fff" stroke-opacity=".3" stroke-width="2"/>
      <circle cx="170" cy="80" r="35" fill="#fff" fill-opacity=".15"/><path d="M-20 230L70 70l45 100 60-65 90 145v90H0z" fill="#081a1a" fill-opacity=".5"/>
      <path d="M-20 270L100 135l60 120 90-90v170H0z" fill="#041211" fill-opacity=".55"/><rect width="240" height="320" fill="url(#s)"/>
      {text}
      <text x="120" y="302" text-anchor="middle" fill="#fff" fill-opacity=".35" font-family="sans-serif" font-size="8">MOCK COLLECTION · {index + 1:03}</text></svg>''')

GLYPHS = {
    "all": '<path d="M3 3h7v7H3zm11 0h7v7h-7zM3 14h7v7H3zm11 0h7v7h-7z"/>',
    "installed": '<circle cx="12" cy="12" r="9"/><path d="m7 12 3 3 7-7"/>',
    "cloud": '<path d="M7 18a5 5 0 1 1 1-10 6 6 0 0 1 11 2 4 4 0 0 1-1 8z"/>',
    "globe": '<circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/>',
    "this": '<path d="M3 4h18v13H3zm5 17h8m-4-4v4"/>',
    "other": '<path d="M5 4h14l3 16H2zm-2 12h18"/>',
    "sync": '<path d="M5 9a8 8 0 0 1 13-3l2 2m0-5v5h-5M19 15a8 8 0 0 1-13 3l-2-2m0 5v-5h5"/>',
    "menu": '<path d="M4 6h16M4 12h16M4 18h16"/>',
    "minimize": '<path d="M6 12h12"/>',
    "maximize": '<path d="M6 6h12v12H6z"/>',
    "close": '<path d="m6 6 12 12M6 18 18 6"/>',
    "search": '<circle cx="10" cy="10" r="7"/><path d="m15 15 6 6"/>',
    "play": '<path fill="#172b22" stroke="none" d="M6 3v18l15-9z"/>',
    "external": '<path d="M13 3h8v8m0-8L10 14M9 5H3v16h16v-6"/>',
    "more": '<path stroke-width="3.5" stroke-linecap="round" d="M12 4v.1M12 12v.1M12 20v.1"/>',
    "windows": '<path fill="#00a4ef" stroke="none" d="m1 3 10-1v9H1zm12-1 10-1v10H13zM1 13h10v9L1 21zm12 0h10v10l-10-1z"/>',
    "xbox": '<circle fill="#16832c" stroke="none" cx="12" cy="12" r="11"/><path stroke="#7ce68d" stroke-width="3" d="m5 5 14 14M19 5 5 19"/>',
}


@lru_cache(maxsize=32)
def glyph(name):
    path = Path(__file__).parent / "static" / "icons" / ("epicgames.svg" if name == "epic" else name + ".svg")
    if path.exists():
        svg = path.read_text()
    else:
        svg = '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><g fill="none" stroke="#b7c8c0" stroke-width="1.7" stroke-linejoin="round">' + GLYPHS.get(name, GLYPHS["all"]) + '</g></svg>'
    return raster(svg, 64, 64)


def game(index):
    installed = "linux" if index % 5 == 0 else "windows" if index % 4 == 0 else ""
    client = installed or ("windows" if index % 7 == 0 else "linux")
    palette = [QColor.fromHsvF((index * .137 + offset) % 1, .8, .7).name() for offset in [0, .14]]
    return {"id": index, "title": TITLES[index % len(TITLES)] + (f" · {index // len(TITLES) + 1}" if index >= len(TITLES) else ""),
            "store": STORES[(index * 7) % len(STORES)], "installed": installed, "client": client, "colors": palette}


class Library(QObject):
    changed = Signal()

    def __init__(self):
        super().__init__()
        self.catalog = [game(i) for i in range(240)]
        self.visible = self.catalog

    @Property("QVariantList", notify=changed)
    def games(self):
        return self.visible

    @Slot(str, str, str, str)
    def filter(self, query, store, status, os_filter):
        target = {"this": "linux", "other": "windows"}.get(os_filter)
        self.visible = [g for g in self.catalog
                        if query.casefold() in g["title"].casefold()
                        and (not store or g["store"] == store)
                        and (not target or g["client"] == target)
                        and (status == "all" or (bool(g["installed"]) if not target else g["installed"] == target) == (status == "installed"))]
        self.changed.emit()


class Images(QQuickImageProvider):
    def __init__(self):
        super().__init__(QQuickImageProvider.ImageType.Image)

    def requestImage(self, key, size, requested_size):
        kind, value = key.split("/", 1)
        if kind == "icon":
            image = glyph(value)
        else:
            index = int(value)
            data = game(index)
            image = poster(index, data["title"], data["installed"] == "linux")
        size.setWidth(image.width()); size.setHeight(image.height())
        return image


if __name__ == "__main__":
    os.environ.setdefault("QSG_RHI_BACKEND", "vulkan")
    # Select Qt's native inertial wheel path instead of restarting a tween per event.
    os.environ.setdefault("QT_QUICK_FLICKABLE_WHEEL_DECELERATION", "5000")
    app = QGuiApplication(sys.argv)
    app.setApplicationName("Raesir Qt mock")
    app.styleHints().setWheelScrollLines(app.styleHints().wheelScrollLines() * 2)
    engine = QQmlApplicationEngine()
    library = Library()
    engine.rootContext().setContextProperty("library", library)
    engine.addImageProvider("mock", Images())
    engine.load(QUrl.fromLocalFile(str(Path(__file__).with_suffix(".qml").resolve())))
    if not engine.rootObjects():
        raise SystemExit(1)
    raise SystemExit(app.exec())
