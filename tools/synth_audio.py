#!/usr/bin/env python3
"""Procedural sound set for orbit_jumper: vast-open-space ambience,
engine hum, weapons, impacts, UI. Everything synthesized; OGG vorbis out."""
import numpy as np
import soundfile as sf
import os

SR = 22050
OUT = "crates/oj_game/audio"
os.makedirs(OUT, exist_ok=True)
rng = np.random.default_rng(7)

def t_axis(dur):
    return np.arange(int(SR * dur)) / SR

def env_ad(n, attack, decay_tau):
    """Attack (linear, seconds) then exponential decay."""
    e = np.ones(n)
    a = int(attack * SR)
    if a > 0:
        e[:a] = np.linspace(0, 1, a)
    d = np.exp(-np.arange(n - a) / (decay_tau * SR))
    e[a:] *= d
    return e

def fft_lowpass(x, cutoff, order=2.0):
    X = np.fft.rfft(x)
    f = np.fft.rfftfreq(len(x), 1 / SR)
    H = 1.0 / (1.0 + (f / max(cutoff, 1.0)) ** (2 * order))
    return np.fft.irfft(X * H, len(x))

def fft_bandpass(x, lo, hi):
    X = np.fft.rfft(x)
    f = np.fft.rfftfreq(len(x), 1 / SR)
    H = 1.0 / (1.0 + (lo / np.maximum(f, 1.0)) ** 4) * 1.0 / (1.0 + (f / max(hi, 1.0)) ** 4)
    return np.fft.irfft(X * H, len(x))

def swept_lowpass(x, c0, c1, chunk=512):
    """Lowpass whose cutoff glides from c0 to c1 across the signal."""
    y = np.zeros_like(x)
    n = len(x)
    steps = max(n // chunk, 1)
    win = np.hanning(chunk * 2)
    for i in range(steps + 1):
        s = i * chunk
        seg = x[s:s + chunk * 2]
        if len(seg) == 0:
            break
        c = c0 * (c1 / c0) ** (min(s / n, 1.0))
        w = win[: len(seg)]
        y[s:s + len(seg)] += fft_lowpass(seg * w, c)
    return y

def loopify(x, fade=0.25):
    """Crossfade tail into head so the buffer loops seamlessly."""
    nf = int(fade * SR)
    if nf == 0 or nf * 2 > len(x):
        return x
    ramp = np.linspace(0, 1, nf)
    head = x[:nf] * ramp + x[-nf:] * (1 - ramp)
    return np.concatenate([head, x[nf:-nf]])

def norm(x, peak=0.9):
    m = np.max(np.abs(x))
    return x * (peak / m) if m > 0 else x

def write(name, x, peak=0.9):
    x = norm(np.asarray(x), peak)
    sf.write(f"{OUT}/{name}.ogg", x.T if x.ndim == 2 else x, SR,
             format="OGG", subtype="VORBIS")
    kb = os.path.getsize(f"{OUT}/{name}.ogg") / 1024
    print(f"{name:>12}.ogg  {kb:7.1f} KB")

# --- space_drone: 64 s seamless ambient loop, stereo -----------------------
DUR = 64.0
N = int(SR * DUR)
t = np.arange(N) / SR

def q(f):
    """Quantize a frequency to a whole number of cycles per loop."""
    return round(f * DUR) / DUR

def lfo(f, phase=0.0):
    return 0.5 * (1 + np.sin(2 * np.pi * q(f) * t + phase))

def voice(f, amp, lf, lp):
    return amp * np.sin(2 * np.pi * q(f) * t) * lfo(lf, lp)

music = []
for side, detune in ((0, 0.0), (1, 0.35)):
    ch = np.zeros(N)
    # The floor: a deep fifth, breathing at glacier pace.
    ch += voice(55 + detune * 0.2, 0.30, 1 / 64, side * 2.1)
    ch += voice(82.5 + detune * 0.3, 0.20, 2 / 64, 1.0 + side)
    # Pads: detuned cluster, each swelling on its own slow clock.
    ch += voice(220 + detune, 0.10, 3 / 64, side * 0.7)
    ch += voice(275 + detune * 1.4, 0.08, 2 / 64, 2.0 + side * 1.3)
    ch += voice(330 - detune, 0.07, 5 / 64, 4.0 - side)
    # Shimmer: high, faint, drifting in and out like starlight.
    ch += voice(1100 + detune * 4, 0.020, 4 / 64, side * 3.0)
    ch += voice(1650 - detune * 3, 0.012, 7 / 64, 1.5 + side * 2)
    # Air: a soft filtered wash that swells twice per loop.
    wash = fft_lowpass(rng.standard_normal(N), 900) * 0.05 * lfo(2 / 64, side * np.pi)
    ch += wash
    music.append(ch)
# Distant bells: four soft partial strikes per loop, alternating sides.
for i, (at, f0, side) in enumerate([(6, 440, 0), (22, 587, 1), (38, 523, 0), (54, 659, 1)]):
    s = int(at * SR)
    dur_b = 6.0
    nb = int(dur_b * SR)
    tb = np.arange(nb) / SR
    bell = (np.sin(2 * np.pi * f0 * tb) + 0.4 * np.sin(2 * np.pi * f0 * 2.76 * tb)) \
        * np.exp(-tb / 2.2) * 0.035
    end = min(s + nb, N)
    music[side][s:end] += bell[: end - s]
    music[1 - side][s:end] += bell[: end - s] * 0.4
music = np.stack([loopify(m, 1.5) for m in music])
write("space_drone", music, peak=0.8)

# --- engine_loop: 2 s seamless hum --------------------------------------
n = int(2.0 * SR)
tt = np.arange(n) / SR
brown = np.cumsum(rng.standard_normal(n)); brown -= np.linspace(brown[0], brown[-1], n)
brown = fft_lowpass(brown / np.max(np.abs(brown)), 340)
hum = 0.5 * np.sin(2 * np.pi * 62 * tt) * (1 + 0.12 * np.sin(2 * np.pi * 2.0 * tt))
write("engine_loop", loopify(brown * 0.9 + hum, 0.3), peak=0.7)

# --- laser ---------------------------------------------------------------
tt = t_axis(0.22); n = len(tt)
f = 1500 * (240 / 1500) ** (tt / tt[-1])
ph = 2 * np.pi * np.cumsum(f) / SR
zap = np.sin(ph) + 0.35 * np.sign(np.sin(ph * 2)) * 0.5
zap *= env_ad(n, 0.004, 0.06)
zap += fft_bandpass(rng.standard_normal(n), 2000, 6000) * env_ad(n, 0.001, 0.02) * 0.2
write("laser", zap)

# --- missile: rising whoosh ---------------------------------------------
n = int(0.7 * SR)
wh = swept_lowpass(rng.standard_normal(n), 500, 2400)
wh = fft_bandpass(wh, 220, 3000)
e = np.sin(np.pi * np.arange(n) / n) ** 1.5
write("missile", wh * e, peak=0.7)

# --- explosion -----------------------------------------------------------
n = int(1.1 * SR); tt = np.arange(n) / SR
boom = swept_lowpass(rng.standard_normal(n), 2600, 150) * env_ad(n, 0.002, 0.28)
thump = np.sin(2 * np.pi * 55 * tt * np.exp(-tt * 0.8)) * env_ad(n, 0.001, 0.22) * 0.9
write("explosion", boom + thump)

# --- shield_hit: energetic shimmer --------------------------------------
n = int(0.5 * SR); tt = np.arange(n) / SR
sh = np.zeros(n)
for f0, a in [(420, 0.5), (633, 0.4), (947, 0.3), (1310, 0.22)]:
    vib = 1 + 0.006 * np.sin(2 * np.pi * 9 * tt)
    sh += a * np.sin(2 * np.pi * f0 * vib * tt) * np.exp(-tt / 0.14)
sh += fft_bandpass(rng.standard_normal(n), 1500, 5000) * env_ad(n, 0.001, 0.03) * 0.3
write("shield_hit", sh)

# --- hull_hit: dull thud -------------------------------------------------
n = int(0.32 * SR); tt = np.arange(n) / SR
f = 95 * (48 / 95) ** (tt / tt[-1])
thud = np.sin(2 * np.pi * np.cumsum(f) / SR) * env_ad(n, 0.002, 0.09)
thud += fft_lowpass(rng.standard_normal(n), 1200) * env_ad(n, 0.0, 0.012) * 0.5
write("hull_hit", thud)

# --- click: soft UI blip -------------------------------------------------
n = int(0.07 * SR); tt = np.arange(n) / SR
blip = (np.sin(2 * np.pi * 950 * tt) + 0.3 * np.sin(2 * np.pi * 1900 * tt)) * env_ad(n, 0.002, 0.02)
write("click", blip, peak=0.6)

# --- orbit_lock: two-note confirmation ----------------------------------
n = int(0.55 * SR); tt = np.arange(n) / SR
a = np.sin(2 * np.pi * 587 * tt) * env_ad(n, 0.01, 0.12)
nb = int(0.18 * SR)
b = np.zeros(n)
tb = np.arange(n - nb) / SR
b[nb:] = np.sin(2 * np.pi * 880 * tb) * env_ad(n - nb, 0.01, 0.16)
write("orbit_lock", (a * 0.6 + b) * 0.8, peak=0.65)

# --- salvage: rising pickup arp ------------------------------------------
n = int(0.4 * SR)
arp = np.zeros(n)
for i, f0 in enumerate([880, 1108, 1318]):
    s = int(i * 0.09 * SR)
    ns = int(0.22 * SR)
    ts = np.arange(ns) / SR
    seg = np.sin(2 * np.pi * f0 * ts) * env_ad(ns, 0.004, 0.06)
    end = min(s + ns, n)
    arp[s:end] += seg[: end - s] * 0.7
write("salvage", arp, peak=0.55)

# --- solar_arm: servo whirr + clunk --------------------------------------
n = int(0.9 * SR); tt = np.arange(n) / SR
f = 140 + (420 - 140) * tt / tt[-1]
saw = 2 * ((np.cumsum(f) / SR) % 1.0) - 1
servo = fft_lowpass(saw * (1 + 0.5 * np.sin(2 * np.pi * 28 * tt)), 900) * 0.5
servo *= np.minimum(1, np.linspace(0, 8, n))
clunk_n = int(0.1 * SR)
clunk = fft_lowpass(rng.standard_normal(clunk_n), 500) * env_ad(clunk_n, 0.0, 0.03)
servo[-clunk_n:] += clunk * 1.4
write("solar_arm", servo, peak=0.55)

# --- warning: two urgent beeps ------------------------------------------
n = int(0.7 * SR)
warn = np.zeros(n)
for s0 in (0.0, 0.32):
    s = int(s0 * SR); nb = int(0.18 * SR)
    tb = np.arange(nb) / SR
    beep = np.tanh(2.2 * np.sin(2 * np.pi * 392 * tb)) * env_ad(nb, 0.008, 0.3)
    warn[s:s + nb] += beep
write("warning", warn, peak=0.6)

print("done")
