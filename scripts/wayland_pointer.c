/* Persistent virtual pointer for the private Wayland GUI smoke test.
 * Generated protocol bindings are built into target/, never into the app.
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <wayland-client.h>
#include "virtual-pointer.h"

static struct zwlr_virtual_pointer_manager_v1 *manager;

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    (void)version;
    if (strcmp(interface, "zwlr_virtual_pointer_manager_v1") == 0) {
        manager = wl_registry_bind(registry, name,
                    &zwlr_virtual_pointer_manager_v1_interface, 1);
    }
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}
static const struct wl_registry_listener listener = {global, removed};

int main(void) {
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !manager) return 2;
    struct zwlr_virtual_pointer_v1 *pointer =
        zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    if (wl_display_roundtrip(display) < 0) return 3;
    usleep(200000);  /* allow clients to bind the new wl_pointer capability */
    puts("ready"); fflush(stdout);
    unsigned int x, y, button;
    while (scanf("%u %u %u", &x, &y, &button) == 3) {
        zwlr_virtual_pointer_v1_motion_absolute(pointer, 0, x, y, 1600, 1000);
        zwlr_virtual_pointer_v1_frame(pointer);
        if (wl_display_roundtrip(display) < 0) return 4;
        usleep(50000);  /* pointer enter must precede the first button event */
        /* 10/11/12 hold, move and release for a genuine drag sequence. */
        uint32_t code = button == 3 ? 0x111 : 0x110; /* Linux BTN_RIGHT/LEFT */
        if (button != 11 && button != 12)
            zwlr_virtual_pointer_v1_button(pointer, 0, code, WL_POINTER_BUTTON_STATE_PRESSED);
        if (button != 10 && button != 11)
            zwlr_virtual_pointer_v1_button(pointer, 0, code, WL_POINTER_BUTTON_STATE_RELEASED);
        zwlr_virtual_pointer_v1_frame(pointer);
        if (wl_display_roundtrip(display) < 0) return 5;
        puts("clicked"); fflush(stdout);
    }
    zwlr_virtual_pointer_v1_destroy(pointer);
    zwlr_virtual_pointer_manager_v1_destroy(manager);
    wl_registry_destroy(registry);
    wl_display_disconnect(display);
    return 0;
}
