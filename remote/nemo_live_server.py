"""NeMo streaming diarization service for the remote GPU host.

Run with the isolated ``nemo-live`` Python environment.  The existing
pyannote service remains on port 8000; this service uses port 8001.
"""

import io
import secrets
import threading
import time
import wave
from dataclasses import dataclass
from pathlib import Path

import torch
from fastapi import FastAPI, File, Header, HTTPException, UploadFile
from nemo.collections.asr.models import SortformerEncLabelModel


def api_key():
    for line in Path(r"C:\Users\Focky\pyannote-3.1\.env").read_text().splitlines():
        key, _, value = line.partition("=")
        if key == "API_KEY":
            return value
    raise RuntimeError("API_KEY is missing")


def authorize(value):
    if not value or not secrets.compare_digest(value, api_key()):
        raise HTTPException(401, "invalid API key")


def pcm(audio):
    try:
        with wave.open(io.BytesIO(audio), "rb") as wav:
            if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, 16_000):
                raise ValueError("audio must be 16 kHz mono PCM")
            raw = wav.readframes(wav.getnframes())
    except (wave.Error, ValueError) as error:
        raise HTTPException(400, str(error)) from error
    return torch.frombuffer(bytearray(raw), dtype=torch.int16).float().div_(32768)


def segments(predictions, offset_ms, duration_ms):
    frames, speakers = predictions.shape
    if not frames:
        return []
    result = []
    for speaker in range(speakers):
        start = None
        for frame in range(frames + 1):
            active = frame < frames and predictions[frame, speaker] >= 0.5
            if active and start is None:
                start = frame
            elif not active and start is not None:
                begin = offset_ms + round(start * duration_ms / frames)
                end = offset_ms + round(frame * duration_ms / frames)
                if end - begin >= 160:
                    result.append({"start": begin / 1000, "end": end / 1000, "speaker": speaker})
                start = None
    return sorted(result, key=lambda item: (item["start"], item["speaker"]))


@dataclass
class Session:
    state: object
    seen: float


app = FastAPI()
model = SortformerEncLabelModel.from_pretrained("nvidia/diar_streaming_sortformer_4spk-v2.1").eval()
sessions = {}
lock = threading.Lock()  # ponytail: serial GPU inference; add a queue only if sessions contend.


@app.get("/health")
def health():
    return {"model": "nvidia/diar_streaming_sortformer_4spk-v2.1", "device": torch.cuda.get_device_name(0)}


@app.post("/live")
def live(
    audio: UploadFile = File(...),
    session: str = "",
    offset_ms: int = 0,
    x_api_key: str | None = Header(default=None),
):
    authorize(x_api_key)
    if not session or len(session) > 128 or not session.replace("-", "").isalnum():
        raise HTTPException(400, "invalid session")
    samples = pcm(audio.file.read())
    if not len(samples):
        return {"segments": []}
    with lock, torch.inference_mode():
        now = time.monotonic()
        for key in [key for key, value in sessions.items() if now - value.seen > 600]:
            del sessions[key]
        current = sessions.get(session)
        if current is None:
            current = Session(
                model.sortformer_modules.init_streaming_state(
                    batch_size=1, async_streaming=model.async_streaming, device=model.device
                ),
                now,
            )
            sessions[session] = current
        signal = samples.unsqueeze(0).to(model.device)
        processed, length = model.process_signal(signal, torch.tensor([len(samples)], device=model.device))
        total = torch.zeros((1, 0, model.sortformer_modules.n_spk), device=model.device)
        current.state, total = model.forward_streaming_step(
            processed.transpose(1, 2), length, current.state, total
        )
        current.seen = now
    return {"segments": segments(total[0].cpu(), offset_ms, round(len(samples) * 1000 / 16_000))}


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host="0.0.0.0", port=8001)
