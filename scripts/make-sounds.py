"""Mailzo's notification sounds, made from nothing but sine waves and filtered noise.

Run with: python3 scripts/make-sounds.py app/src-tauri/sounds (needs numpy)
"""
import sys, wave
import numpy as np

SR = 44100
LOUDNESS_DB = -18
OUT = sys.argv[1]
rng = np.random.default_rng(7)

def t_for(sec):
    return np.arange(int(SR * sec)) / SR

def note(freq, sec, partials, attack=0.003):
    """partials: (ratio, level, decay seconds). Struck things lose their highs first."""
    t = t_for(sec)
    out = np.zeros_like(t)
    for ratio, level, decay in partials:
        out += level * np.sin(2 * np.pi * freq * ratio * t) * np.exp(-t / decay)
    out *= np.minimum(1, t / attack)
    return out

def fm_note(freq, sec, ratio=1.0, index=1.2, decay=0.45, index_decay=0.12, attack=0.004):
    """Electric piano: a soft sine with a brief bright edge at the start."""
    t = t_for(sec)
    mod = index * np.exp(-t / index_decay) * np.sin(2 * np.pi * freq * ratio * t)
    out = np.sin(2 * np.pi * freq * t + mod) * np.exp(-t / decay)
    return out * np.minimum(1, t / attack)

def pluck(freq, sec, damp=0.5, brightness=0.45):
    """A plucked string (Karplus-Strong) started from softened noise."""
    n = int(SR / freq)
    line = rng.uniform(-1, 1, n)
    for _ in range(3):
        line = brightness * line + (1 - brightness) * np.roll(line, 1)
    out = np.zeros(int(SR * sec))
    for i in range(len(out)):
        out[i] = line[i % n]
        line[i % n] = damp * line[i % n] + (1 - damp) * line[(i + 1) % n]
        line[i % n] *= 0.9985
    return out

def at(track, sound, when, gain=1.0):
    start = int(SR * when)
    end = min(len(track), start + len(sound))
    track[start:end] += gain * sound[: end - start]

def room(x, wet=0.12, decay=0.16, length=0.35):
    """A small, soft room, so nothing sounds like it's coming out of a tin."""
    t = t_for(length)
    tail = rng.normal(0, 1, len(t)) * np.exp(-t / decay)
    tail = np.convolve(tail, np.ones(14) / 14, mode="same")
    tail /= np.sqrt((tail**2).sum())
    wetted = np.convolve(x, tail)[: len(x)]
    return x + wet * wetted

def one_pole(x, cutoff):
    a = np.exp(-2 * np.pi * cutoff / SR)
    y = np.zeros_like(x)
    prev = 0.0
    for i, v in enumerate(x):
        prev = (1 - a) * v + a * prev
        y[i] = prev
    return y

def finish(x):
    x = x - one_pole(x, 180)          # laptop speakers only rattle below this
    x = x - x.mean()
    fade = int(SR * 0.04)
    x[-fade:] *= np.linspace(1, 0, fade) ** 2
    # Level with the Mac's own sounds, by how loud they feel rather than by their highest point.
    win = int(SR * 0.05)
    loudest = max(np.sqrt(np.mean(x[i:i + win] ** 2)) for i in range(0, len(x) - win, win // 2))
    return x * min(10 ** (LOUDNESS_DB / 20) / loudest, 0.7 / np.abs(x).max())

BELL = [(1, 1.0, 0.42), (2.0, 0.28, 0.22), (3.01, 0.10, 0.10), (4.2, 0.05, 0.05)]
MARIMBA = [(1, 1.0, 0.20), (3.93, 0.26, 0.035), (9.2, 0.05, 0.012)]
GLASS = [(1, 1.0, 0.55), (2.32, 0.34, 0.30), (4.25, 0.18, 0.16), (6.63, 0.07, 0.07)]

def chime():
    track = np.zeros(int(SR * 0.95))
    at(track, note(1046.5, 0.9, BELL), 0.0, 0.8)       # C6
    at(track, note(1318.5, 0.8, BELL), 0.11, 1.0)      # E6
    return room(track)

def marimba():
    track = np.zeros(int(SR * 0.6))
    at(track, note(784.0, 0.55, MARIMBA, attack=0.002), 0.0, 0.85)    # G5
    at(track, note(1046.5, 0.5, MARIMBA, attack=0.002), 0.095, 1.0)   # C6
    return room(track, wet=0.16)

def droplet():
    track = np.zeros(int(SR * 0.45))
    def drop(start, end, sec):
        t = t_for(sec)
        freq = start * (end / start) ** (np.minimum(t / 0.07, 1))
        phase = 2 * np.pi * np.cumsum(freq) / SR
        return np.sin(phase) * np.exp(-t / 0.055) * np.minimum(1, t / 0.002)
    at(track, drop(620, 1180, 0.3), 0.0, 1.0)
    at(track, drop(760, 1480, 0.25), 0.105, 0.55)
    return room(track, wet=0.2)

def pluck_sound():
    track = np.zeros(int(SR * 0.85))
    at(track, pluck(659.3, 0.8), 0.0, 0.8)     # E5
    at(track, pluck(987.8, 0.7), 0.1, 1.0)     # B5
    return room(one_pole(track, 5200), wet=0.14)

def glass():
    track = np.zeros(int(SR * 1.0))
    at(track, note(1567.98, 0.98, GLASS, attack=0.0015), 0.0, 1.0)   # G6
    return room(track, wet=0.2, decay=0.22)

def keys():
    track = np.zeros(int(SR * 0.9))
    for when, freq, gain in [(0.0, 523.25, 0.7), (0.075, 659.25, 0.8), (0.15, 783.99, 1.0)]:
        at(track, fm_note(freq, 0.75, ratio=2.0, index=0.9), when, gain)
    return room(track, wet=0.16)

SOUNDS = {"Chime": chime, "Marimba": marimba, "Droplet": droplet, "Pluck": pluck_sound, "Glass": glass, "Keys": keys}
# Droplet and Pluck were tried and turned down. They're still made because each sound draws on the
# same random noise, so leaving them out would change the ones that were chosen.
KEPT = {"Chime", "Marimba", "Glass", "Keys"}
for name, make in SOUNDS.items():
    x = finish(make())
    if name not in KEPT:
        continue
    pcm = (np.clip(x, -1, 1) * 32767).astype("<i2")
    with wave.open(f"{OUT}/{name}.wav", "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR); w.writeframes(pcm.tobytes())
