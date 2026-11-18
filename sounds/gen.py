"""Genera los sonidos de la isla (WAV 44,1 kHz, 16 bits, mono) sin archivos de terceros.
Uso: python3 sounds/gen.py  -> escribe los .wav en ui/sounds/.
Timbre común: campanita suave (seno + armónicos que se apagan antes) con un poco de eco."""

import math, struct, wave
from pathlib import Path

RATE = 44100
OUT = Path(__file__).parent.parent / "ui" / "sounds"


def midi(n):  # 69 = La4 (440 Hz)
    return 440 * 2 ** ((n - 69) / 12)


def note(buf, n, at, dur=0.35, amp=1.0, bright=0.35, glide=0.0):
    """Suma una nota a buf: ataque de 5 ms y caída exponencial. glide = semitonos que sube o baja."""
    f0 = midi(n)
    start = int(at * RATE)
    length = int(dur * RATE)
    if len(buf) < start + length:
        buf.extend([0.0] * (start + length - len(buf)))
    phase = 0.0
    for i in range(length):
        t = i / RATE
        f = f0 * 2 ** (glide * min(t / 0.06, 1) / 12)
        phase += 2 * math.pi * f / RATE
        env = min(t / 0.005, 1) * math.exp(-t * 5.5 / dur)
        s = math.sin(phase) + bright * math.sin(2 * phase) * math.exp(-t * 18) + bright * 0.3 * math.sin(3 * phase) * math.exp(-t * 30)
        buf[start + i] += amp * env * s


def tick(buf, at, amp=1.0, f=2400):
    """Clic corto: un seno agudo de ~12 ms."""
    start = int(at * RATE)
    length = int(0.012 * RATE)
    if len(buf) < start + length:
        buf.extend([0.0] * (start + length - len(buf)))
    for i in range(length):
        t = i / RATE
        buf[start + i] += amp * math.sin(2 * math.pi * f * t) * math.exp(-t * 400)


def whoosh(buf, f_from, f_to, dur, amp=1.0):
    """Barrido grave y suave (abrir/cerrar el notch)."""
    length = int(dur * RATE)
    buf.extend([0.0] * max(0, length - len(buf)))
    phase = 0.0
    for i in range(length):
        x = i / length
        f = f_from * (f_to / f_from) ** x
        phase += 2 * math.pi * f / RATE
        buf[i] += amp * math.sin(phase) * math.sin(math.pi * x) ** 2


def write(name, buf, peak=0.32, echo=True):
    buf = buf + [0.0] * int(0.25 * RATE)
    if echo:  # eco corto y tenue, da "aire" sin sonar a reverb
        for d, g in ((0.071, 0.18), (0.113, 0.12)):
            k = int(d * RATE)
            for i in range(len(buf) - 1, k - 1, -1):
                buf[i] += g * buf[i - k]
    # recorta el silencio final y suaviza los últimos 20 ms
    top = max(abs(s) for s in buf)
    end = max(i for i, s in enumerate(buf) if abs(s) > top * 0.002) + 1
    buf = buf[:end]
    fade = min(int(0.02 * RATE), len(buf))
    for i in range(fade):
        buf[-1 - i] *= i / fade
    scale = peak / top
    (OUT / name).parent.mkdir(exist_ok=True)
    with wave.open(str(OUT / f"{name}.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1, min(1, s * scale)) * 32767)) for s in buf))
    print(f"{name}.wav  {len(buf) / RATE:.2f} s")


def make():
    # C5 = 72, E5 = 76, G5 = 79, A5 = 81, C6 = 84, E6 = 88
    b = []; note(b, 72, 0, 0.7, 0.7, 0.2); note(b, 79, 0.0, 0.7, 0.5, 0.2)
    write("notificacion", b)  # llega una notificación

    b = []; note(b, 72, 0, 0.35, 0.8); note(b, 76, 0.1, 0.35, 0.8); note(b, 79, 0.2, 0.35, 0.85); note(b, 84, 0.3, 0.7)
    write("claude-termino", b)  # Claude terminó

    b = []; [note(b, 76, i * 0.13, 0.14, 0.9, 0.3) for i in range(3)]
    write("claude-permiso", b, peak=0.36)  # Claude pide permiso: toc-toc-toc

    b = []; note(b, 74, 0, 0.3, 0.8); note(b, 81, 0.12, 0.55)
    write("focus-inicio", b)  # empieza la concentración

    b = []; note(b, 84, 0, 0.35, 0.8); note(b, 79, 0.13, 0.35, 0.8); note(b, 76, 0.26, 0.6, 0.9)
    write("focus-descanso", b)  # empieza un descanso (bajando, relaja)

    b = []; note(b, 72, 0, 0.6, 0.6); note(b, 76, 0, 0.6, 0.6); note(b, 79, 0, 0.6, 0.6); note(b, 84, 0.18, 0.9, 0.8)
    write("focus-fin", b)  # se termina el focus

    b = []; note(b, 69, 0, 0.3, 0.9, 0.2); note(b, 64, 0.17, 0.5, 0.9, 0.2)
    write("bateria-baja", b)  # batería baja (grave, bajando)

    b = []; note(b, 76, 0, 0.25, 0.7, glide=7)
    write("cargando", b)  # se conecta el cargador

    b = []; note(b, 79, 0, 0.18, 0.8); note(b, 86, 0.08, 0.35)
    write("bluetooth-conectado", b)

    b = []; note(b, 86, 0, 0.18, 0.8); note(b, 79, 0.08, 0.35, 0.8)
    write("bluetooth-desconectado", b)

    b = []; [note(b, n, i * 0.05, 0.3, 0.5 + i * 0.12, 0.2) for i, n in enumerate((84, 88, 91, 96))]
    write("drop-listo", b, peak=0.26)  # QR listo para escanear (destello)

    b = []; tick(b, 0, 0.6, 1800); note(b, 83, 0.02, 0.4)
    write("archivo-recibido", b)  # llegó un archivo desde el teléfono

    b = []; whoosh(b, 180, 420, 0.16)
    write("abrir", b, peak=0.14, echo=False)  # el notch se despliega (muy sutil)

    b = []; whoosh(b, 380, 170, 0.14)
    write("cerrar", b, peak=0.12, echo=False)  # el notch se recoge

    b = []; note(b, 67, 0, 0.22, 0.7, 0.2); note(b, 74, 0.08, 0.4, 0.9, 0.2)
    write("sonido-activado", b)  # se quita el silencio (al silenciar no suena: Windows ya está en mute)

    b = []; tick(b, 0)
    write("clic", b, peak=0.18, echo=False)  # botones (play/pausa, anterior/siguiente, pastillas)


if __name__ == "__main__":
    make()
