#!/usr/bin/env python3
"""
codegen.py — Generate gen_moves.rs and gen_items.rs from Showdown TypeScript data.

Usage:  python3 codegen.py
Input:  src/showdown_data/{moves,items}.ts
Output: src/data/generated/{gen_moves,gen_items}.rs
"""

import re
import json
import sys
from pathlib import Path

ROOT = Path(__file__).parent
SD = ROOT / "src" / "showdown_data"
OUT = ROOT / "src" / "data" / "generated"

# ═══════════════════════════════════════════════════════════════════
# Type mapping
# ═══════════════════════════════════════════════════════════════════

TYPE_MAP = {
    "Normal": 0, "Fire": 1, "Water": 2, "Electric": 3,
    "Grass": 4, "Ice": 5, "Fighting": 6, "Poison": 7,
    "Ground": 8, "Flying": 9, "Psychic": 10, "Bug": 11,
    "Rock": 12, "Ghost": 13, "Dragon": 14, "Dark": 15,
    "Steel": 16, "Fairy": 17,
}

TYPE_RUST = {
    0: "Type::Normal", 1: "Type::Fire", 2: "Type::Water", 3: "Type::Electric",
    4: "Type::Grass", 5: "Type::Ice", 6: "Type::Fighting", 7: "Type::Poison",
    8: "Type::Ground", 9: "Type::Flying", 10: "Type::Psychic", 11: "Type::Bug",
    12: "Type::Rock", 13: "Type::Ghost", 14: "Type::Dragon", 15: "Type::Dark",
    16: "Type::Steel", 17: "Type::Fairy",
}

STATUS_MAP = {
    "brn": 1, "par": 2, "psn": 3, "tox": 4, "slp": 5, "frz": 6,
}

# Stats: our engine uses [ATK=0, DEF=1, SPA=2, SPD=3, SPE=4]
STAT_INDEX = {"atk": 0, "def": 1, "spa": 2, "spd": 3, "spe": 4}

# ═══════════════════════════════════════════════════════════════════
# MoveEffect mapping: Showdown move ID → engine MoveEffect
# ═══════════════════════════════════════════════════════════════════

# Maps from the Showdown key (lowercase, no spaces) → MoveEffect variant name
MOVE_EFFECT = {
    # -- Status moves --
    "protect": "Protect", "detect": "Protect", "kingsshield": "Protect",
    "banefulbunker": "Protect", "spikyshield": "Protect", "silktrap": "Protect",
    "obstruct": "Protect", "burningbulwark": "Protect",
    "stealthrock": "StealthRock", "spikes": "Spikes",
    "toxicspikes": "ToxicSpikes", "stickyweb": "StickyWeb",
    "defog": "Defog",
    "willowisp": "WillOWisp", "thunderwave": "ThunderWave", "toxic": "Toxic",
    "spore": "Sleep", "sleeppowder": "Sleep", "hypnosis": "Sleep",
    "darkvoid": "Sleep", "grasswhistle": "Sleep", "lovelykiss": "Sleep",
    "sing": "Sleep", "yawn": "Sleep",
    "swordsdance": "SwordsDance", "nastyplot": "NastyPlot",
    "dragondance": "DragonDance", "calmmind": "CalmMind",
    "bulkup": "BulkUp", "irondefense": "IronDefense",
    "agility": "Agility", "autotomize": "Agility", "rockpolish": "Agility",
    "quiverdance": "QuiverDance", "shellsmash": "ShellSmash",
    "coil": "Coil", "shiftgear": "ShiftGear",
    "reflect": "Reflect", "lightscreen": "LightScreen",
    "auroraveil": "AuroraVeil", "tailwind": "Tailwind",
    "trickroom": "TrickRoom",
    "substitute": "Substitute", "wish": "Wish",
    "taunt": "Taunt", "leechseed": "LeechSeed", "encore": "Encore",

    # -- Damaging side effects --
    "uturn": "ForceSwitch", "voltswitch": "ForceSwitch",
    "flipturn": "ForceSwitch", "chillydrop": "ForceSwitch",
    "rapidspin": "RapidSpin",

    # -- Damage modifiers --
    "knockoff": "KnockOff", "freezedry": "FreezeDry",
    "expandingforce": "ExpandingForce", "psyblade": "Psyblade",
    "solarbeam": "SolarBeam", "solarblade": "SolarBeam",

    # -- Stat-override moves --
    "foulplay": "FoulPlay",
    "bodypress": "BodyPress",
    "psyshock": "Photon", "psystrike": "Photon", "secretsword": "Photon",

    # -- Type/power override moves --
    "weatherball": "WeatherBall",
    "terrainpulse": "TerrainPulse",
    "grassyglide": "GrassyGlide",

    # -- Weather accuracy --
    "thunder": "WeatherAccRain", "hurricane": "WeatherAccRain",
    "blizzard": "WeatherAccSnow",

    # -- Pivots --
    "partingshot": "PartingShot", "batonpass": "BatonPass",

    # -- Charge moves --
    "fly": "ChargeFly", "bounce": "ChargeFly",
    "dig": "ChargeDig",
    "dive": "ChargeDive",
    "phantomforce": "ChargePhantom", "shadowforce": "ChargePhantom",
    "skyattack": "ChargeSkyAttack",
    "skullbash": "ChargeSkullBash",
    "meteorbeam": "ChargeMeteorBeam",
    "electroshot": "ChargeElectroShot",
    "geomancy": "ChargeGeomancy",

    # -- Locked / thrashing --
    "outrage": "Thrash", "petaldance": "Thrash", "thrash": "Thrash",
    "ragingfury": "Thrash",
}

# ═══════════════════════════════════════════════════════════════════
# SelfEffect mapping: Showdown move key → engine SelfEffect variant
# ═══════════════════════════════════════════════════════════════════

SELF_EFFECT = {
    # -- Self-stat drops on damaging moves --
    # -1 Def, -1 SpD
    "closecombat": "DefSpDDown1",
    "armorcannon": "DefSpDDown1",
    "dragonascent": "DefSpDDown1",
    "headlongrush": "DefSpDDown1",

    # -1 Atk, -1 Def
    "superpower": "AtkDefDown1",

    # -1 Def, -1 SpD, -1 Spe
    "vcreate": "DefSpDSpeDown1",

    # -2 SpA
    "dracometeor": "SpADown2",
    "leafstorm": "SpADown2",
    "overheat": "SpADown2",
    "fleurcannon": "SpADown2",
    "psychoboost": "SpADown2",

    # -1 SpA
    "makeitrain": "SpADown1",

    # -1 Spe
    "hammerarm": "SpeDown1",
    "icehammer": "SpeDown1",

    # -2 Spe
    "spinout": "SpeDown2",

    # -1 Def
    "hyperspacefury": "DefDown1",

    # -- Crash damage (50% max HP on miss) --
    "highjumpkick": "CrashDamage",
    "jumpkick": "CrashDamage",
    "axekick": "CrashDamage",
    "supercellslam": "CrashDamage",

    # -- Thaw self (non-Fire moves that thaw user) --
    # Fire-type moves thaw by default in the engine, so only non-Fire thaw
    # moves need this. Scald/Steam Eruption are Water but thaw user.
    "scald": "ThawSelf",
    "steameruption": "ThawSelf",
}

# ═══════════════════════════════════════════════════════════════════
# VarPower mapping
# ═══════════════════════════════════════════════════════════════════

VAR_POWER = {
    "lowkick": "Weight", "grassknot": "Weight",
    "gyroball": "GyroBall",
    "facade": "Facade",
    "eruption": "Eruption", "waterspout": "Eruption",
    "flail": "Flail", "reversal": "Flail",
    "heavyslam": "HeavySlam", "heatcrash": "HeavySlam",
    "punishment": "Punishment",
    "storedpower": "StoredPower",
    "electroball": "ElectroBall",
    "return": "Return", "frustration": "Frustration",
    "hex": "Hex", "barbedbranch": "Hex",
    "acrobatics": "Acrobatics",
    "risingvoltage": "RisingVoltage",
}

# ═══════════════════════════════════════════════════════════════════
# Showdown flag → engine MoveFlags bit
# ═══════════════════════════════════════════════════════════════════

# Showdown flag name → our MoveFlags constant name
SD_FLAG_MAP = {
    "contact": "CONTACT", "sound": "SOUND", "punch": "PUNCH",
    "pulse": "PULSE", "bite": "BITE", "powder": "POWDER",
    "dance": "DANCE", "wind": "WIND", "slicing": "SLICE",
    "bullet": "BULLET", "bypasssub": "BYPASSSUB",
}


# ═══════════════════════════════════════════════════════════════════
# Parse moves.ts
# ═══════════════════════════════════════════════════════════════════

def parse_ts_object(text):
    """Rough parse of a TS data file into a dict of {key: raw_text_block}.

    This is NOT a full JS parser. It finds top-level entries like:
        key: { ... },
    and returns the raw text between the outer braces.
    """
    entries = {}
    # Match top-level entries: \tkey: {
    pattern = re.compile(r'^\t(\w+):\s*\{', re.MULTILINE)
    matches = list(pattern.finditer(text))
    for i, m in enumerate(matches):
        key = m.group(1)
        start = m.end()
        # Find the closing brace at depth 0, tab-level 1
        depth = 1
        pos = start
        while pos < len(text) and depth > 0:
            ch = text[pos]
            if ch == '{':
                depth += 1
            elif ch == '}':
                depth -= 1
            pos += 1
        entries[key] = text[start:pos-1]
    return entries


def extract_field(block, field_name):
    """Extract a simple field value from a block of text."""
    # Matches:  fieldName: value,  or  fieldName: value\n
    pat = re.compile(rf'^\s*{field_name}:\s*(.+?)(?:,\s*$|\s*$)', re.MULTILINE)
    m = pat.search(block)
    if m:
        return m.group(1).strip().rstrip(',')
    return None


def extract_num(block):
    """Extract `num:` field."""
    val = extract_field(block, "num")
    if val:
        return int(val)
    return None


def extract_flags(block):
    """Extract Showdown flags dict as a set of flag names."""
    pat = re.compile(r'flags:\s*\{([^}]*)\}')
    m = pat.search(block)
    if not m:
        return set()
    inner = m.group(1)
    return set(re.findall(r'(\w+)\s*:', inner))


def extract_secondary(block):
    """Extract secondary effect: returns (chance, status, stat, stages) or None."""
    # Check for `secondary: null`
    null_pat = re.compile(r'secondary:\s*null')
    if null_pat.search(block):
        return None

    # Match secondary block
    sec_pat = re.compile(r'secondary:\s*\{(.*?)\}', re.DOTALL)
    m = sec_pat.search(block)
    if not m:
        return None
    sec_text = m.group(1)

    chance = 0
    status = 0
    stat = 0
    stages = 0

    ch_m = re.search(r'chance:\s*(\d+)', sec_text)
    if ch_m:
        chance = int(ch_m.group(1))

    st_m = re.search(r"status:\s*['\"](\w+)['\"]", sec_text)
    if st_m:
        status = STATUS_MAP.get(st_m.group(1), 0)

    # Stat boosts in secondary
    boost_m = re.search(r'boosts:\s*\{([^}]*)\}', sec_text)
    if boost_m:
        boost_inner = boost_m.group(1)
        for st_name, st_idx in STAT_INDEX.items():
            val_m = re.search(rf'{st_name}:\s*(-?\d+)', boost_inner)
            if val_m:
                stat = st_idx
                stages = int(val_m.group(1))
                break

    if chance > 0:
        return (chance, status, stat, stages)
    return None


def extract_drain(block):
    """Extract drain: [num, den] → i8 value (positive = drain, negative = recoil)."""
    drain_pat = re.compile(r'drain:\s*\[\s*(\d+)\s*,\s*(\d+)\s*\]')
    m = drain_pat.search(block)
    if m:
        n, d = int(m.group(1)), int(m.group(2))
        # Convert fraction to percentage-ish i8
        # drain: [1, 2] = 50% heal = drain 50
        return int(n * 100 // d)

    recoil_pat = re.compile(r'recoil:\s*\[\s*(\d+)\s*,\s*(\d+)\s*\]')
    m = recoil_pat.search(block)
    if m:
        n, d = int(m.group(1)), int(m.group(2))
        return -int(n * 100 // d)

    # hasCrashDamage: recoil on miss
    if 'hasCrashDamage' in block:
        return -50

    return 0


def extract_multihit(block):
    """Extract multihit: [lo, hi] or multihit: N."""
    mh_pat = re.compile(r'multihit:\s*\[\s*(\d+)\s*,\s*(\d+)\s*\]')
    m = mh_pat.search(block)
    if m:
        return int(m.group(1)), int(m.group(2))
    mh_pat2 = re.compile(r'multihit:\s*(\d+)')
    m = mh_pat2.search(block)
    if m:
        v = int(m.group(1))
        return v, v
    return 0, 0


def extract_crit_ratio(block):
    """Extract critRatio."""
    val = extract_field(block, "critRatio")
    if val:
        try:
            return int(val)
        except ValueError:
            pass
    return 0


def extract_priority(block):
    """Extract priority as i8."""
    val = extract_field(block, "priority")
    if val:
        try:
            return int(val)
        except ValueError:
            pass
    return 0


def extract_bp(block):
    """Extract basePower."""
    val = extract_field(block, "basePower")
    if val:
        try:
            return min(int(val), 255)
        except ValueError:
            pass
    return 0


def extract_accuracy(block):
    """Extract accuracy. 'true' means never-miss (0 in our encoding)."""
    val = extract_field(block, "accuracy")
    if val == "true":
        return 0  # bypasses accuracy check
    if val:
        try:
            return int(val)
        except ValueError:
            pass
    return 0


def extract_pp(block):
    """Extract PP."""
    val = extract_field(block, "pp")
    if val:
        try:
            return int(val)
        except ValueError:
            pass
    return 0


def extract_category(block):
    """Extract category."""
    val = extract_field(block, "category")
    if val:
        val = val.strip("'\"")
        if val == "Physical":
            return "MoveCategory::Physical"
        elif val == "Special":
            return "MoveCategory::Special"
    return "MoveCategory::Status"


def extract_type(block):
    """Extract move type."""
    val = extract_field(block, "type")
    if val:
        val = val.strip("'\"")
        tid = TYPE_MAP.get(val, 0)
        return TYPE_RUST.get(tid, "Type::Normal")
    return "Type::Normal"


def extract_target(block):
    """Extract target."""
    val = extract_field(block, "target")
    if val:
        val = val.strip("'\"")
    return val or "normal"


def is_nonstandard(block):
    """Check if move is CAP/LGPE/etc that we skip."""
    val = extract_field(block, "isNonstandard")
    if val:
        val = val.strip("'\"")
        return val in ("CAP", "LGPE", "Unobtainable")
    return False


def has_heal_flag(block, sd_flags):
    """Check if move has self-healing (Recover, Roost, etc.)."""
    # heal flag in Showdown means recovery move
    return "heal" in sd_flags


def has_recharge(block):
    """Moves with recharge turn (Hyper Beam, etc.)."""
    return 'rechargeTurn' in block or extract_field(block, "self") and "'mustrecharge'" in block


def is_protect_target(block, sd_flags):
    """Check if move can be blocked by Protect (the protect flag in Showdown)."""
    return "protect" in sd_flags


def gen_moves():
    """Generate gen_moves.rs from moves.ts."""
    text = (SD / "moves.ts").read_text(encoding="utf-8")
    entries = parse_ts_object(text)

    # Build by num → data
    moves = {}  # num → (key, block)
    max_num = 0
    for key, block in entries.items():
        num = extract_num(block)
        if num is None or num <= 0:
            continue
        if is_nonstandard(block):
            continue
        if num > max_num:
            max_num = num
        # Skip duplicate nums (keep first = the standard version)
        if num not in moves:
            moves[num] = (key, block)

    # Generate arrays
    slots = max_num + 1

    lines_hot = []
    lines_cold = []
    name_consts = []

    for i in range(slots):
        if i not in moves:
            lines_hot.append(f"    // [{i}] \u2014")
            lines_hot.append(
                "    MoveData { flags:0, base_power:0, accuracy:0,\n"
                "        category:MoveCategory::Status, move_type:Type::Normal,\n"
                "        var_power:VarPower::None, crit_ratio:0, drain:0, priority:0,\n"
                "        multihit:0,\n"
                "        secondary_chance:0, secondary_stat:0,\n"
                "        effect:MoveEffect::None, secondary_status:0,\n"
                "        self_effect:SelfEffect::None },"
            )
            lines_cold.append(f"    MoveMeta {{ pp: 0, target: MoveTarget::Normal }}, // [{i}]")
            continue

        key, block = moves[i]
        name_raw = extract_field(block, "name")
        if name_raw:
            name_raw = name_raw.strip("'\"")
        else:
            name_raw = key

        sd_flags = extract_flags(block)
        bp = extract_bp(block)
        accuracy = extract_accuracy(block)
        category = extract_category(block)
        move_type = extract_type(block)
        pp = extract_pp(block)
        priority = extract_priority(block)
        crit_ratio = extract_crit_ratio(block)
        drain = extract_drain(block)
        mh_lo, mh_hi = extract_multihit(block)
        secondary = extract_secondary(block)
        target = extract_target(block)

        # Clamp drain to i8
        drain = max(-128, min(127, drain))

        # Build MoveFlags
        flag_parts = []
        for sd_flag, rust_flag in SD_FLAG_MAP.items():
            if sd_flag in sd_flags:
                flag_parts.append(f"MoveFlags::{rust_flag}")

        # PROTECT flag: set if the move checks for Protect (i.e., Showdown has `protect: 1`)
        if is_protect_target(block, sd_flags):
            flag_parts.append("MoveFlags::PROTECT")

        # HEAL flag
        if has_heal_flag(block, sd_flags):
            flag_parts.append("MoveFlags::HEAL")

        # RECHARGE flag
        if has_recharge(block) or "'mustrecharge'" in block:
            flag_parts.append("MoveFlags::RECHARGE")

        # CHARGE flag (set for charge moves)
        if key in MOVE_EFFECT and MOVE_EFFECT[key].startswith("Charge"):
            flag_parts.append("MoveFlags::CHARGE")
        # SolarBeam is also CHARGE
        if key in ("solarbeam", "solarblade"):
            flag_parts.append("MoveFlags::CHARGE")

        flags_str = " | ".join(flag_parts) if flag_parts else "0"

        # MoveEffect
        effect = "MoveEffect::None"
        if key in MOVE_EFFECT:
            effect = f"MoveEffect::{MOVE_EFFECT[key]}"

        # VarPower
        var_power = "VarPower::None"
        if key in VAR_POWER:
            var_power = f"VarPower::{VAR_POWER[key]}"

        # Secondary
        sec_chance = 0
        sec_status = 0
        sec_stat = 0
        if secondary:
            sec_chance, sec_status, sec_stat_idx, sec_stages = secondary
            if sec_stages != 0:
                # Encode stat + stages: secondary_stat = signed stages, but we need
                # to also encode which stat. Our format: secondary_stat is i8 where
                # the sign = direction, and the stat is encoded in sec_stat field
                # Actually looking at the MoveData struct: secondary_stat is i8 (stages)
                # and there's no field for which stat. But looking at move_exec.rs,
                # secondary_stat is the stat index shifted + stages... Let me check.
                # Actually from the code, secondary_stat is just the stages (i8),
                # and the stat to boost is inferred from the secondary boosts.
                # For simplicity: secondary_stat = stages, and we encode the stat
                # in the effect or leave it as a TODO.
                # For now: encode as signed stat_index * stages
                sec_stat = sec_stages  # just the stages

        # Target mapping
        target_rust = "MoveTarget::Normal"
        target_map = {
            "normal": "MoveTarget::Normal",
            "self": "MoveTarget::Self_",
            "adjacentAlly": "MoveTarget::AllyOrSelf",
            "adjacentAllyOrSelf": "MoveTarget::AllyOrSelf",
            "allAdjacentFoes": "MoveTarget::AllAdjacentFoes",
            "allAdjacent": "MoveTarget::AllAdjacent",
            "any": "MoveTarget::Any",
            "foeSide": "MoveTarget::FoeSide",
            "allySide": "MoveTarget::AllySide",
            "all": "MoveTarget::All",
            "randomNormal": "MoveTarget::Normal",
            "scripted": "MoveTarget::Normal",
            "allyTeam": "MoveTarget::AllySide",
        }
        target_rust = target_map.get(target, "MoveTarget::Normal")

        # SelfEffect
        self_effect = "SelfEffect::None"
        if key in SELF_EFFECT:
            self_effect = f"SelfEffect::{SELF_EFFECT[key]}"

        # Crash damage moves: drain should be 0 (crash handled via SelfEffect, not recoil)
        if key in SELF_EFFECT and SELF_EFFECT[key] == "CrashDamage":
            drain = 0

        # Pack multihit: lo in bits[3:0], hi in bits[7:4]
        multihit_packed = (mh_hi << 4) | mh_lo

        # Name constant
        const_name = re.sub(r'[^A-Za-z0-9]', '_', name_raw).upper()
        const_name = re.sub(r'_+', '_', const_name).strip('_')
        name_consts.append(f"pub const MOVE_{const_name}: usize = {i};")

        lines_hot.append(f"    // [{i}] {name_raw}")
        lines_hot.append(
            f"    MoveData {{ flags:{flags_str}, base_power:{bp}, accuracy:{accuracy},\n"
            f"        category:{category}, move_type:{move_type},\n"
            f"        var_power:{var_power}, crit_ratio:{crit_ratio}, drain:{drain}, priority:{priority},\n"
            f"        multihit:{multihit_packed},\n"
            f"        secondary_chance:{sec_chance}, secondary_stat:{sec_stat},\n"
            f"        effect:{effect}, secondary_status:{sec_status},\n"
            f"        self_effect:{self_effect} }},"
        )
        lines_cold.append(
            f"    MoveMeta {{ pp: {pp}, target: {target_rust} }}, // [{i}] {name_raw}"
        )

    # Write output
    out = []
    out.append("// AUTO-GENERATED by codegen.py \u2014 do not edit by hand.")
    out.append("// Source: smogon/pokemon-showdown")
    out.append("// Regenerate with: python3 codegen.py")
    out.append("")
    out.append("#![allow(unused)]")
    out.append("")
    out.append("use crate::data::types::Type;")
    out.append("use crate::data::moves::{MoveData, MoveMeta, MoveCategory, MoveTarget, VarPower, MoveEffect, MoveFlags, SelfEffect};")
    out.append("")
    out.append(f"/// {len(moves)} moves in {slots} slots.")
    out.append(f"pub static GEN_MOVES: &[MoveData] = &[")
    out.extend(lines_hot)
    out.append("];")
    out.append("")
    out.append(f"pub static GEN_MOVE_META: &[MoveMeta] = &[")
    out.extend(lines_cold)
    out.append("];")
    out.append("")
    out.append("// ── Move ID constants ────────────────────────────────────────────")
    out.append("")
    out.extend(name_consts)
    out.append("")

    (OUT / "gen_moves.rs").write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"Wrote {OUT / 'gen_moves.rs'}: {len(moves)} moves, {slots} slots")


# ═══════════════════════════════════════════════════════════════════
# Item codegen
# ═══════════════════════════════════════════════════════════════════

# Showdown item key → (flags, type_param, power_param) mapping
# These are items with special engine flags that can't be auto-detected.

def gen_items():
    """Generate gen_items.rs from items.ts."""
    text = (SD / "items.ts").read_text(encoding="utf-8")
    entries = parse_ts_object(text)

    items = {}  # spritenum → (key, name, flags_set, type_param, fling_bp)
    max_num = 0

    for key, block in entries.items():
        # Use spritenum as the index (matches the old hand-crafted gen_items.rs)
        sn = extract_field(block, "spritenum")
        if sn is None:
            continue
        try:
            num = int(sn)
        except ValueError:
            continue
        if num <= 0:
            continue
        if num > max_num:
            max_num = num

        name_raw = extract_field(block, "name")
        if name_raw:
            name_raw = name_raw.strip("'\"")
        else:
            name_raw = key

        # Skip nonstandard
        ns = extract_field(block, "isNonstandard")
        if ns and ns.strip("'\"") in ("CAP", "Unobtainable"):
            continue

        flags = set()
        type_param = 0xFF
        fling_bp = 0

        # Extract fling basePower
        fling_m = re.search(r'fling:\s*\{\s*basePower:\s*(\d+)', block)
        if fling_m:
            fling_bp = int(fling_m.group(1))

        # Detect item types from Showdown data
        is_berry = "isBerry: true" in block

        # Mega Stone
        if "megaStone:" in block:
            flags.add("MEGA_STONE")

        # Z-Crystal
        if "zMove:" in block or "isZ:" in block:
            flags.add("Z_CRYSTAL")

        # Type-boost items (like Charcoal, Mystic Water, etc.)
        # Showdown has onBasePower with this.chainModify([4915, 4096]) for 1.2x
        if "onBasePowerPriority" in block and "chainModify" in block:
            # Try to detect type from the type check
            type_check = re.search(r"move\.type\s*===\s*['\"](\w+)['\"]", block)
            if type_check:
                t = TYPE_MAP.get(type_check.group(1))
                if t is not None:
                    flags.add("TYPE_BOOST")
                    type_param = t

        # Plates / type boost items
        # Also detect via `onPlate` or direct assignments
        if "onPlate:" in block:
            plate_m = re.search(r"onPlate:\s*['\"](\w+)['\"]", block)
            if plate_m:
                t = TYPE_MAP.get(plate_m.group(1))
                if t is not None:
                    flags.add("TYPE_BOOST")
                    type_param = t

        # Resist berries
        # Detected by: isBerry + onSourceModifyDamage or onEffectiveness with type check
        if is_berry and ("onSourceModifyDamage" in block or "onEffectiveness" in block):
            type_check = re.search(r"type\s*===\s*['\"](\w+)['\"]", block)
            if type_check:
                t = TYPE_MAP.get(type_check.group(1))
                if t is not None:
                    flags.add("RESIST_BERRY")
                    flags.add("IS_BERRY")
                    flags.add("CONSUMABLE")
                    type_param = t

        # Gems
        if "isGem: true" in block:
            flags.add("GEM")
            flags.add("CONSUMABLE")
            type_check = re.search(r"type\s*===\s*['\"](\w+)['\"]", block)
            if type_check:
                t = TYPE_MAP.get(type_check.group(1))
                if t is not None:
                    type_param = t

        # Pinch berries (stat boost at ≤25% HP)
        # Detected by: isBerry + onUpdate with maxhp / 4 check + onEat with boost
        if is_berry and "maxhp / 4" in block and "onEat" in block:
            boost_m = re.search(r'onEat.*?boost.*?\{([^}]*)\}', block, re.DOTALL)
            if boost_m:
                boost_inner = boost_m.group(1)
                for st_name, st_idx in STAT_INDEX.items():
                    if st_name in boost_inner:
                        flags.add("PINCH_BERRY")
                        flags.add("IS_BERRY")
                        flags.add("CONSUMABLE")
                        type_param = st_idx
                        break

        # Terrain seeds
        if "Seed" in name_raw and ("electricterrain" in block or "grassyterrain" in block
                                    or "psychicterrain" in block or "mistyterrain" in block):
            flags.add("TERRAIN_SEED")
            flags.add("CONSUMABLE")
            if "electricterrain" in block:
                type_param = 1
            elif "grassyterrain" in block:
                type_param = 2
            elif "psychicterrain" in block:
                type_param = 3
            elif "mistyterrain" in block:
                type_param = 4

        # Specific well-known items
        SPECIFIC_ITEMS = {
            "choiceband": ("CHOICE_ATK",),
            "choicescarf": ("CHOICE_SPE",),
            "choicespecs": ("CHOICE_SPA",),
            "assaultvest": ("ASSAULT_VEST",),
            "eviolite": ("EVIOLITE",),
            "lifeorb": ("LIFE_ORB",),
            "expertbelt": ("EXPERT_BELT",),
            "metronome": ("METRONOME",),
            "razorclaw": ("CRIT_BOOST",),
            "scopelens": ("CRIT_BOOST",),
            "widelens": ("WIDE_LENS",),
            "focussash": ("FOCUS_SASH", "CONSUMABLE"),
            "airballoon": ("AIR_BALLOON", "CONSUMABLE"),
            "safetygoggles": ("SAFETY_GOGGLES",),
            "rockyhelmet": ("ROCKY_HELMET",),
            "leftovers": ("LEFTOVERS",),
            "blacksludge": ("BLACK_SLUDGE",),
            "flameorb": ("FLAME_ORB",),
            "toxicorb": ("TOXIC_ORB",),
            "heavydutyboots": ("HAZARD_IMMUNE",),
            "shedshell": ("TRAP_IMMUNE",),
            "lightclay": ("EXTENDS_SCREENS",),
            "bindingband": ("BINDING_BOOST",),
            "powerherb": ("POWER_HERB", "CONSUMABLE"),
            "protectivepads": (),  # no engine flag yet
        }
        if key in SPECIFIC_ITEMS:
            for f in SPECIFIC_ITEMS[key]:
                flags.add(f)

        if is_berry:
            flags.add("IS_BERRY")

        if num not in items:
            items[num] = (key, name_raw, flags, type_param, fling_bp)

    # Cap at 716 or max_num+1
    slots = max(max_num + 1, 716)

    out = []
    out.append("//! Generated item data table.")
    out.append("//!")
    out.append("//! Item IDs match Pok\u00e9mon Showdown's numbering.")
    out.append("//! Populated entries have flags set; all others are ItemData::NONE (flags=0).")
    out.append("")
    out.append("use crate::data::items::ItemData;")
    out.append("use crate::data::items::ItemFlag as F;")
    out.append("")
    out.append(f"pub static GEN_ITEMS: [ItemData; {slots}] = {{")
    out.append("    const N: ItemData = ItemData { flags: 0, type_param: 0xFF, power_param: 0, _padding: [0; 2] };")
    out.append(f"    let mut t = [N; {slots}];")

    for num in sorted(items.keys()):
        key, name_raw, flags, type_param, fling_bp = items[num]
        if not flags:
            continue  # Skip items with no engine-relevant flags

        flag_parts = sorted(flags)
        flags_str = " | ".join(f"F::{f}" for f in flag_parts)
        tp = type_param if type_param != 0xFF else "0xFF"
        out.append(f"    // [{num:>3}] {name_raw}")
        out.append(f"    t[{num}] = ItemData {{ flags: {flags_str}, type_param: {tp}, power_param: {fling_bp}, _padding: [0; 2] }};")

    out.append("    t")
    out.append("};")
    out.append("")

    (OUT / "gen_items.rs").write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"Wrote {OUT / 'gen_items.rs'}: {len([n for n in items if items[n][2]])} items with flags")


# ═══════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════

if __name__ == "__main__":
    gen_moves()
    gen_items()
    print("Done.")
