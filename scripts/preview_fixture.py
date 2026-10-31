"""Shared native-preview fixture and pixel assertions (private test displays only)."""
from PIL import Image

RED = (224, 40, 30)
BLUE = (30, 70, 240)


def create(directory, color=RED):
    directory.mkdir(exist_ok=True)
    Image.new("RGB", (64, 32), color).save(directory / "a.png")
    (directory / "b.txt").write_text("native text preview after image\n")


def bounds(image, color):
    # Right-hand preview in a 1360x860 window with 8x16 cells.
    crop = image.convert("RGB").crop((1072, 200, 1352, 760))
    mask = Image.frombytes("L", crop.size, bytes(255 if p == color else 0 for p in crop.getdata()))
    return mask.getbbox()


def assert_aspect(image, color):
    box = bounds(image, color)
    assert box, f"No preview pixels for {color}"
    width, height = box[2] - box[0], box[3] - box[1]
    assert width >= 200 and abs(width - height * 2) <= 2, f"Preview aspect changed: {box}"
