"""HTTP server the Rust desktop app talks to.

Run from the node/ folder:
    python -m groundtruth.node

Endpoints:
    GET  /health    -> {"status": "ok", ...}   (lets the app check the node is up)
    POST /identify  -> ComponentLabels JSON     (multipart form, field name "image")
"""

import os

import uvicorn
from dotenv import load_dotenv
from fastapi import FastAPI, File, HTTPException, UploadFile

# Load GEMINI_API_KEY (and friends) from the .env file at the repo root.
load_dotenv()

from .schema import Component, ComponentLabels  # noqa: E402

MOCK = os.getenv("GROUNDTRUTH_MOCK") == "1"

app = FastAPI(title="Ground Truth node")


@app.get("/health")
def health():
    return {
        "status": "ok",
        "mock": MOCK,
        "gemini_key_loaded": bool(os.getenv("GEMINI_API_KEY")),
    }


@app.post("/identify", response_model=ComponentLabels)
async def identify(image: UploadFile = File(...)):
    image_bytes = await image.read()
    if not image_bytes:
        raise HTTPException(status_code=400, detail="Empty image upload.")

    if MOCK:
        # Fake answer so you can test the Rust <-> Python link without spending API calls.
        return ComponentLabels(
            components=[
                Component(ref="R1", kind="resistor", value="220 ohm",
                          color_bands=["red", "red", "brown", "gold"], confidence=0.9),
                Component(ref="LED1", kind="led", value="red",
                          polarity_note="long leg is anode (+)", confidence=0.95),
            ],
            notes=f"MOCK response for {image.filename} ({len(image_bytes)} bytes)",
            source="mock",
        )

    from .gemini_labeler import label_components

    try:
        return label_components(image_bytes, image.content_type or "image/jpeg")
    except Exception as e:  # surface the real reason to the desktop app
        raise HTTPException(status_code=502, detail=f"Gemini call failed: {e}")


def main():
    host = os.getenv("GROUNDTRUTH_HOST", "127.0.0.1")
    port = int(os.getenv("GROUNDTRUTH_PORT", "8000"))
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()