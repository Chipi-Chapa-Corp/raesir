"""Cross-platform checks for the shipped application identity and artwork."""

import configparser
import json
import re
from pathlib import Path
import struct
import tomllib
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parent.parent
APP_ID = "com.chipichapa.exposedgames"


def config(path):
    parser = configparser.ConfigParser(interpolation=None)
    parser.optionxform = str
    parser.read(path)
    return parser


class BrandingTests(unittest.TestCase):
    def test_linux_identity_agrees_across_metadata(self):
        storage = (ROOT / "backend/src/storage.rs").read_text()
        native_id = re.search(r'pub const APP_ID: &str = "([^"]+)";', storage)
        self.assertIsNotNone(native_id)
        self.assertEqual(native_id.group(1), APP_ID)
        manifest = json.loads((ROOT / "build" / f"{APP_ID}.json").read_text())
        self.assertEqual(manifest["app-id"], APP_ID)
        self.assertEqual(manifest["command"], "exposed-games")
        desktop = config(ROOT / "build" / f"{APP_ID}.desktop")["Desktop Entry"]
        self.assertEqual(desktop["Name"], "Exposed Games")
        self.assertEqual(desktop["Exec"], "exposed-games")
        self.assertEqual(desktop["Icon"], APP_ID)
        self.assertEqual(desktop["StartupWMClass"], APP_ID)
        appstream = ET.parse(ROOT / "build" / f"{APP_ID}.metainfo.xml")
        self.assertEqual(appstream.findtext("id"), APP_ID)
        self.assertEqual(appstream.findtext("name"), "Exposed Games")
        self.assertEqual(appstream.findtext("launchable"), f"{APP_ID}.desktop")
        ref = config(ROOT / "build/exposed-games.flatpakref")["Flatpak Ref"]
        self.assertEqual(ref["Name"], APP_ID)
        self.assertEqual(ref["Title"], "Exposed Games")

    def test_executable_names(self):
        native = tomllib.loads((ROOT / "native/Cargo.toml").read_text())
        backend = tomllib.loads((ROOT / "backend/Cargo.toml").read_text())
        self.assertEqual(native["package"]["name"], "exposed-games")
        self.assertEqual(native["bin"][0]["name"], "exposed-games")
        self.assertEqual(backend["package"]["name"], "exposed-games-backend")
        self.assertEqual(backend["bin"][0]["name"], "exposed-games-scan")

    def test_icon_is_self_contained_vector_art(self):
        svg = ET.parse(ROOT / "build/icon.svg").getroot()
        ns = {"s": "http://www.w3.org/2000/svg"}
        self.assertFalse(svg.findall(".//s:image", ns))
        self.assertEqual((svg.get("width"), svg.get("height")), ("256", "256"))
        for gradient in svg.findall("s:defs/s:linearGradient", ns):
            self.assertEqual(gradient.get("x1"), gradient.get("x2"))

    def test_png_and_windows_icon_cover_supported_sizes(self):
        png = (ROOT / "build/icon.png").read_bytes()
        self.assertEqual(png[:8], b"\x89PNG\r\n\x1a\n")
        self.assertEqual(struct.unpack(">IIBB", png[16:26]), (256, 256, 8, 6))
        ico = (ROOT / "build/icon.ico").read_bytes()
        reserved, kind, count = struct.unpack_from("<HHH", ico)
        self.assertEqual((reserved, kind), (0, 1))
        sizes = set()
        for index in range(count):
            width, height, _, _, _, _, length, offset = struct.unpack_from("<BBBBHHII", ico, 6 + 16 * index)
            self.assertEqual(width, height)
            self.assertLessEqual(offset + length, len(ico))
            sizes.add(width or 256)
        self.assertTrue({16, 20, 24, 32, 40, 48, 64, 128, 256}.issubset(sizes))


if __name__ == "__main__":
    unittest.main()
