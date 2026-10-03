"""Shared component-ID schema.

Both the online path (Gemini) and the offline path (YOLO) must return this
exact shape, so the Rust app never has to care which one ran.

Note: there are deliberately no positions/coordinates here. Vision models only
label parts; OpenCV grid mapping owns all geometry.
"""

from typing import List, Literal, Optional

from pydantic import BaseModel, Field

ComponentKind = Literal[
    "resistor",
    "led",
    "capacitor",
    "diode",
    "transistor",
    "ic",
    "button",
    "potentiometer",
    "jumper_wire",
    "other",
]


class Component(BaseModel):
    ref: str = Field(description="Short reference name, e.g. R1, LED1, C1, U1.")
    kind: ComponentKind = Field(description="What type of part this is.")
    value: Optional[str] = Field(
        default=None,
        description="Electrical value if readable, e.g. '220 ohm', '10 uF'. Null if unknown.",
    )
    color_bands: Optional[List[str]] = Field(
        default=None,
        description="Resistor color bands in order, e.g. ['red','red','brown','gold']. Null if not a resistor.",
    )
    markings: Optional[str] = Field(
        default=None,
        description="Text printed on the part, e.g. 'NE555P'. Null if none.",
    )
    polarity_note: Optional[str] = Field(
        default=None,
        description="For polarized parts: how to tell orientation, e.g. 'long leg is anode'. Null otherwise.",
    )
    confidence: float = Field(description="0.0 to 1.0, how sure you are about this label.")


class ComponentLabels(BaseModel):
    components: List[Component] = Field(description="Every part visible on the breadboard.")
    notes: Optional[str] = Field(
        default=None,
        description="Anything that made identification hard (blur, glare, parts hidden). Null if none.",
    )
    source: str = Field(default="gemini", description="Which labeler produced this: 'gemini', 'yolo', or 'mock'.")