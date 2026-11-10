#!/usr/bin/env python3
"""Builds dict/en.txt, the word list behind st-cli's spell checker.

Source: the Hunspell en_US dictionary (the .dic word list and the .aff affix
rules), expanded into plain words. Output: lowercase words (letters and
apostrophes only; short abbreviations pruned), one per line, sorted by byte
value and without duplicates,
so that `src/spell.rs` can binary-search the embedded text without loading it.

Usage: python3 tools/gen_dict.py [en_US.dic en_US.aff]
"""
import re
import sys

dic_path = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/hunspell/en_US.dic"
aff_path = sys.argv[2] if len(sys.argv) > 2 else "/usr/share/hunspell/en_US.aff"

# Everyday spellings a book dictionary leaves out (chat and roleplay talk).
EXTRA = """
ew ok okay yeah yep yup nope nah hmm hmmm mhm uhm umm uh um ugh ahh aww awww ooh oooh
hah haha hehe heh huh oof whoa woah wow yay hey hiya heya gonna wanna gotta kinda
sorta dunno gimme lemme gotcha ya y'all ain't cuz coz bye lol omg btw fyi
blushes smirks smirk grins chuckles giggles sighs shrugs nods hums gasps whispers
sniffles snickers scoffs huffs pouts winks smiles frowns grumbles mumbles mutters
emoji emojis selfie selfies podcast podcasts website websites email emails online
offline smartphone smartphones wifi app apps username usernames login logins
café cafés cliché clichés résumé résumés naïve naïvely fiancé fiancée protégé façade
touché déjà entrée crème brûlée señor señora jalapeño piñata mañana über
""".split()

# ---- affix rules
rules = {"PFX": {}, "SFX": {}}  # kind -> flag -> (cross, [(strip, add, regex)])
only_compound = None
for line in open(aff_path, encoding="utf-8"):
    line = line.split("#")[0].rstrip() if not line.startswith("#") else ""
    if not line.strip():
        continue
    p = line.split()
    if p[0] == "ONLYINCOMPOUND":
        only_compound = p[1]
    elif p[0] in ("PFX", "SFX"):
        kind, flag = p[0], p[1]
        if len(p) == 4:  # header: PFX A Y 1
            rules[kind][flag] = (p[2] == "Y", [])
        else:  # rule: PFX A strip add cond
            strip = "" if p[2] == "0" else p[2]
            add = "" if p[3] == "0" else p[3].split("/")[0]
            cond = p[4] if len(p) > 4 else "."
            rx = re.compile(cond + "$" if kind == "SFX" else "^" + cond)
            rules[kind][flag][1].append((strip, add, rx))

def apply(kind, flag, word):
    out = []
    cross, lst = rules[kind].get(flag, (False, []))
    for strip, add, rx in lst:
        if not rx.search(word):
            continue
        if kind == "SFX":
            if strip and not word.endswith(strip):
                continue
            out.append((word[: len(word) - len(strip)] + add, cross))
        else:
            if strip and not word.startswith(strip):
                continue
            out.append((add + word[len(strip):], cross))
    return out

words = set()
with open(dic_path, encoding="utf-8") as f:
    next(f)  # entry count
    for line in f:
        tok = line.strip().split()
        if not tok:
            continue
        entry = tok[0]
        flags = ""
        if "/" in entry:
            entry, flags = entry.split("/", 1)
        if only_compound and only_compound in flags:
            continue
        forms = {entry}
        pfx = [x for fl in flags for x in apply("PFX", fl, entry)]
        sfx = [x for fl in flags for x in apply("SFX", fl, entry)]
        forms.update(w for w, _ in pfx)
        forms.update(w for w, _ in sfx)
        # A prefix and a suffix combine when both allow it.
        for fp in flags:
            if fp not in rules["PFX"] or not rules["PFX"][fp][0]:
                continue
            for pw, _ in apply("PFX", fp, entry):
                for fs in flags:
                    if fs in rules["SFX"] and rules["SFX"][fs][0]:
                        forms.update(w for w, _ in apply("SFX", fs, pw))
        words.update(forms)

words.update(EXTRA)
ok = re.compile(r"^[^\W\d_]+(?:'[^\W\d_]+)*$")
clean = set()
for w in words:
    w = w.replace("\u2019", "'").lower()
    if len(w) >= 1 and ok.match(w):
        clean.add(w)
# The Hunspell list is full of abbreviations, state codes, units and symbols
# that make real typos pass (`ho`, `hx`, `cpl`). Short words are therefore
# pruned: one letter only a / i / o, two letters only the list below, and three
# letters need a vowel (except a few interjections).
ONE = set("aio")
TWO = set(
    "ad ah am an as at aw ax be by do dr eh er ew ex go ha he hi hm if in is it "
    "ma me mm mr ms my no of oh ok on or ow ox pa pi pm re so to tv uh um up "
    "us we ya ye yo".split()
)
NO_VOWEL_3 = set("hmm shh brr tsk nth pst psh".split())

def keep(w):
    if "'" in w:
        return True
    n = len(w)
    if n == 1:
        return w in ONE
    if n == 2:
        return w in TWO
    if n == 3:
        return bool(set("aeiouy") & set(w)) or w in NO_VOWEL_3
    return True

out = sorted((w for w in clean if keep(w)), key=lambda s: s.encode("utf-8"))
with open("dict/en.txt", "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(out) + "\n")
print(len(out), "words")
