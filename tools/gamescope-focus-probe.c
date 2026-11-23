/*
 * Synthetic focus probe for Gamescope in Steam integration mode.
 * Run on a disposable Gamescope XWayland display only. This creates two
 * windows with fixture app IDs and checks focus as the second window opens
 * and closes. It does not represent Aurora, Steam, or controller input.
 */
#define _POSIX_C_SOURCE 200809L
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <stdio.h>
#include <time.h>

static Window map_window(Display *display, Atom steam_game, const char *title,
                         unsigned long app_id)
{
    Window window = XCreateSimpleWindow(display, DefaultRootWindow(display),
                                        10, 10, 640, 480, 0, 0, 0);
    XStoreName(display, window, title);
    XChangeProperty(display, window, steam_game, XA_CARDINAL, 32,
                    PropModeReplace, (const unsigned char *)&app_id, 1);
    XMapWindow(display, window);
    XFlush(display);
    return window;
}

static int expect_focus(Display *display, const char *stage, Window expected)
{
    const struct timespec poll_delay = { .tv_sec = 0, .tv_nsec = 50000000 };
    Window actual = None;
    int revert;
    for (int attempt = 0; attempt < 60; attempt++) {
        XGetInputFocus(display, &actual, &revert);
        if (actual == expected) {
            printf("%s: focus reached 0x%lx\n", stage, actual);
            return 1;
        }
        nanosleep(&poll_delay, NULL);
    }
    fprintf(stderr, "%s: expected focus 0x%lx, observed 0x%lx\n",
            stage, expected, actual);
    return 0;
}

int main(void)
{
    Display *display = XOpenDisplay(NULL);
    if (!display) {
        fputs("Could not open the Gamescope XWayland display.\n", stderr);
        return 2;
    }

    Atom steam_game = XInternAtom(display, "STEAM_GAME", False);
    Window shell = map_window(display, steam_game, "Shell fixture", 769);
    if (!expect_focus(display, "shell", shell)) {
        XCloseDisplay(display);
        return 1;
    }

    Window game = map_window(display, steam_game, "Game fixture", 12345);
    int game_focused = expect_focus(display, "game", game);
    XDestroyWindow(display, game);
    XFlush(display);
    int shell_restored = expect_focus(display, "shell restored", shell);
    XCloseDisplay(display);
    return game_focused && shell_restored ? 0 : 1;
}
