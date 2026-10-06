#!/usr/bin/env python3
"""Emit play_check steps that drive Optimist into a busy song:
a 2-bar drum groove (free take at 90 bpm), a bass line on track 1,
7th chords on track 2, a 16th lead on track 3, DUST and DUCK up.
Usage: busy_song.py > busy.steps   (one step per line)"""

BEAT = 60 / 90  # Optimist's power-on tempo
E8 = BEAT / 2
E16 = BEAT / 4
PRESS = 0.04
OCT_M, OCT_P, FX, SCL, ENV, LFO, EDIT, GLO, HOME, SAVE, ARP, SEQ, PLAY, REC = range(14)
# White keys F3..G5 (matrix ids 14..40, a semitone per id)
W = [14, 16, 18, 20, 21, 23, 25, 26, 28, 30, 32, 33, 35, 37, 38, 40]
KICK, SNARE, CLAP, HAT, OHAT = W[0], W[2], W[3], W[4], W[5]

steps = []


def run(seconds):
    if seconds > 0:
        steps.append(f"run:{seconds:.4f}")


def tap(ids, length=PRESS, gap=0.0):
    steps.append("hold:" + ",".join(str(i) for i in ids))
    run(length)
    steps.append("release")
    run(gap)


def button(b):
    tap([b], 0.06, 0.25)


steps.append("run:3")  # boot
# Track 4 (drums), free take: 2 bars of eighths.
steps.append("turn:ALGORITHM:3")
run(0.5)
button(REC)
for i in range(16):
    hits = [HAT]
    if i in (0, 4, 8, 11, 12):
        hits.append(KICK)
    if i % 4 == 2:
        hits.append(SNARE)
    if i == 15:
        hits = [OHAT, CLAP]
    tap(hits, PRESS, E8 - PRESS)
tap([REC], 0.06, 1.0)  # closes the loop on the "1"; it plays at once
# Overdub a 16th hat roll with ARP held (ratchets).
tap([REC], 0.06, 0.1)
steps.append(f"hold:{ARP}")
run(0.3)
steps.append(f"hold:{ARP},{W[6]}")
run(BEAT * 8)
steps.append("release")
tap([REC], 0.06, 0.3)


def record_track(turn, notes):
    """turn ALGORITHM, REC (records at once while playing), notes, REC."""
    steps.append(f"turn:ALGORITHM:{turn}")
    run(0.4)
    tap([REC], 0.06, 0.05)
    for ids, length, gap in notes:
        tap(ids, length, gap)
    tap([REC], 0.06, 0.3)


# Track 1: bass, eighths over two bars.
bass = [W[0], W[0], W[7], W[0], W[3], W[3], W[4], W[2]] * 2
record_track(-3, [([n], E8 * 0.7, E8 * 0.3) for n in bass])
# Track 2: chords. SCL + KNOB1 to 7TH, then a chord per half bar.
steps.append("turn:ALGORITHM:1")
run(0.4)
steps.append(f"hold:{SCL}")
run(0.3)
steps.append("turn:KNOB1:3")
run(0.5)
steps.append("release")
run(0.3)
chords = [W[7], W[7], W[10], W[9]] * 2
record_track(0, [([n], BEAT * 1.8, BEAT * 0.2) for n in chords])
# Track 3: 16th lead.
lead = [W[9], W[11], W[12], W[14], W[12], W[11], W[9], W[8]] * 4
record_track(1, [([n], E16 * 0.6, E16 * 0.4) for n in lead])
# Master: DUST and DUCK up (FX + KNOB2 / KNOB3).
steps.append(f"hold:{FX}")
run(0.3)
steps.append("turn:KNOB2:12")
run(0.4)
steps.append("turn:KNOB3:12")
run(0.4)
steps.append("release")
run(1.0)
print("\n".join(steps))
