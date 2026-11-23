"""Exercise keyboard and wheel zoom on the open file canvas."""
import json


def prepare(base, settings, grid):
    source = base / ("zoom-grid" if grid else "zoom-list")
    source.mkdir()
    for i in range(80):
        (source / f"folder-{i:03}").mkdir()
    settings.write_text(json.dumps(dict(grid=grid, preview=False, font_size=16,
                                        icon_mode="images", restore_session=False)))
    return source


def exercise(settings, wheel, key, capture, wait_for):
    # The pointer is outside these samples. Both contain actual rendered controls.
    def regions():
        image = capture().convert("RGB")
        return (image.crop((30*8, 10*16, 95*8, 26*16)).tobytes(),
                image.crop((0, 0, 120*8, 5*16)).tobytes())

    def persisted():
        data = json.loads(settings.read_text())
        assert data["font_size"] == 16, f"Canvas shortcut changed the GUI font to {data['font_size']}"
        return data.get("file_zoom", [0, 0])

    initial, toolbar = regions()
    wheel(True)
    wait_for(lambda: persisted() == [1, 0] and regions()[0] != initial, "one wheel notch enlarges one step")
    enlarged = regions()
    assert persisted()[1] == 0 and enlarged[1] == toolbar
    key("minus", "ctrl")
    wait_for(lambda: persisted() == [0, 0] and regions() == (initial, toolbar), "one key undoes one notch")
    key("equal", "ctrl", "shift")
    wait_for(lambda: persisted() == [1, 0] and regions() == enlarged, "Ctrl+plus matches one wheel notch")
    wheel(False)
    wait_for(lambda: persisted() == [0, 0] and regions() == (initial, toolbar),
             "original canvas and toolbar pixels after decreasing")
    key("equal", "ctrl")
    wait_for(lambda: persisted() == [1, 0] and regions()[0] != initial, "Ctrl+equals alias")
    key("minus", "ctrl")
    wait_for(lambda: regions() == (initial, toolbar) and persisted() == [0, 0], "keyboard round trip")
    key("equal", "ctrl", "alt")
    wait_for(lambda: json.loads(settings.read_text())["font_size"] == 17 and regions()[1] != toolbar,
             "separate GUI font shortcut resizes toolbar")
    key("minus", "ctrl", "alt")
    wait_for(lambda: regions() == (initial, toolbar) and persisted() == [0, 0], "GUI font restored")
