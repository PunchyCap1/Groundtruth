"""Online component ID: send a breadboard photo to Gemini, get labels back as JSON."""

import base64
import os

from google import genai

from .schema import ComponentLabels

MODEL = os.getenv("GEMINI_MODEL", "gemini-3.8-flash")

PROMPT = """You are labeling electronic components in a photo of a breadboard.
List every component you can see (resistors, LEDs, capacitors, chips, buttons, wires).
For resistors, read the color bands and give the value.
For LEDs and other polarized parts, describe how to tell their orientation.
Do NOT judge whether the circuit is correct, and do NOT give positions or row/column numbers.
Only identify the parts."""

_client = None


def _get_client() -> genai.Client:
    # Created lazily so importing this module never needs a key.
    # genai.Client() reads GEMINI_API_KEY from the environment automatically.
    global _client
    if _client is None:
        _client = genai.Client()
    return _client


def label_components(image_bytes: bytes, mime_type: str = "image/jpeg") -> ComponentLabels:
    image_b64 = base64.b64encode(image_bytes).decode("utf-8")

    interaction = _get_client().interactions.create(
        model=MODEL,
        input=[
            {"type": "text", "text": PROMPT},
            {"type": "image", "data": image_b64, "mime_type": mime_type},
        ],
        response_format={
            "type": "text",
            "mime_type": "application/json",
            "schema": ComponentLabels.model_json_schema(),
        },
    )

    labels = ComponentLabels.model_validate_json(interaction.output_text)
    labels.source = "gemini"
    return labels