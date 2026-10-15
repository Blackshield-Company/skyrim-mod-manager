#!/usr/bin/env python3
"""Build a throwaway demo game folder + mod archives for trying the app safely.

Usage: scripts/make-demo.py /tmp/smm-demo
Then:  SMM_GAME_DIR=/tmp/smm-demo/game SMM_PREFIX=/tmp/smm-demo/pfx \
       SMM_INSTANCE=/tmp/smm-demo/instance smm ...
"""
import os, struct, sys, zipfile

def plugin(masters, master=False, light=False):
    sub = b"HEDR" + struct.pack("<HfII", 12, 1.71, 0, 0x800)
    for m in masters:
        z = m.encode() + b"\0"
        sub += b"MAST" + struct.pack("<H", len(z)) + z + b"DATA" + struct.pack("<HQ", 8, 0)
    flags = (1 if master else 0) | (0x200 if light else 0)
    return b"TES4" + struct.pack("<IIIIHH", len(sub), flags, 0, 0, 44, 0) + sub

root = sys.argv[1] if len(sys.argv) > 1 else "/tmp/smm-demo"
data = os.path.join(root, "game", "Data")
os.makedirs(data, exist_ok=True)
open(os.path.join(root, "game", "SkyrimSE.exe"), "wb").close()
for m in ["Skyrim.esm", "Update.esm", "Dawnguard.esm", "HearthFires.esm", "Dragonborn.esm"]:
    open(os.path.join(data, m), "wb").write(plugin([] if m == "Skyrim.esm" else ["Skyrim.esm"], True))
os.makedirs(os.path.join(root, "pfx/drive_c/users/steamuser/AppData/Local/Skyrim Special Edition"), exist_ok=True)

mods = {
    "SkyUI_5_2_SE.zip": {"SkyUI_SE.esp": plugin(["Skyrim.esm"]), "interface/skyui/config.txt": b"a", "interface/startmenu.swf": b"skyui"},
    "Unofficial Patch.zip": {"Data/unofficial skyrim special edition patch.esp": plugin(["Skyrim.esm", "Update.esm", "Dawnguard.esm", "HearthFires.esm", "Dragonborn.esm"], True),
                             "Data/textures/armor/iron/cuirass.dds": b"ussep", "Data/meshes/armor/iron/cuirass.nif": b"ussep"},
    "Iron Armor Retexture.zip": {"Iron Armor Retexture/textures/armor/iron/cuirass.dds": b"hd", "Iron Armor Retexture/textures/armor/iron/gauntlets.dds": b"hd"},
    "Better Armor Meshes.zip": {"meshes/armor/iron/cuirass.nif": b"better", "BetterArmor.esp": plugin(["Skyrim.esm", "Armor Framework.esm"])},
    "Interface Tweaks.zip": {"interface/startmenu.swf": b"tweaks", "fomod/ModuleConfig.xml": b"<config/>"},
}
for name, files in mods.items():
    with zipfile.ZipFile(os.path.join(root, name), "w") as z:
        for p, b in files.items():
            z.writestr(p, b)
print(root)
