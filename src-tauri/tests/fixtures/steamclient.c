/* Test double only. Never links to or starts a real Steam client. */
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#include <stdlib.h>

typedef struct { void **vtable; } Object;
static Object client, user, apps;
static void *client_table[16], *user_table[3], *apps_table[7];
static bool pipe_open = false, user_open = false;
__attribute__((destructor)) static void check_released(void) { if (pipe_open || user_open) abort(); }

static int create_pipe(void *self) { if (pipe_open) return 0; pipe_open = true; return 7; }
static bool release_pipe(void *self, int pipe) { if (pipe != 7 || user_open) abort(); pipe_open = false; return true; }
static int connect_user(void *self, int pipe) { if (pipe != 7 || !pipe_open) abort(); user_open = true; return 8; }
static void release_user(void *self, int pipe, int handle) { if (pipe != 7 || handle != 8) abort(); user_open = false; }
static void *get_user(void *self, int handle, int pipe, const char *version) { return strcmp(version, "SteamUser019") == 0 ? &user : 0; }
static void *get_apps(void *self, int handle, int pipe, const char *version) { return strcmp(version, "STEAMAPPS_INTERFACE_VERSION008") == 0 ? &apps : 0; }
static bool logged_on(void *self) { return user_open; }
static uint64_t steam_id(void *self) { return 76561198000000000ULL; }
static bool subscribed(void *self, uint32_t app) { return app == 42; }

void *CreateInterface(const char *version, int *result) {
    if (strcmp(version, "SteamClient017") != 0) return 0;
    client_table[0] = create_pipe; client_table[1] = release_pipe;
    client_table[2] = connect_user; client_table[4] = release_user;
    client_table[5] = get_user; client_table[15] = get_apps;
    user_table[1] = logged_on; user_table[2] = steam_id;
    apps_table[6] = subscribed;
    client.vtable = client_table; user.vtable = user_table; apps.vtable = apps_table;
    return &client;
}
