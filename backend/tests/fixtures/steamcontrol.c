/* Native Steam control test double; never connects to the real client. */
#define CreateInterface CreateOwnershipInterface
#include "steamclient.c"
#undef CreateInterface
#include <stddef.h>

static Object engine, manager;
static void *engine_table[44], *manager_table[84];
#ifdef _WIN32
/* MSVC x64 RTTI with image-relative offsets; no C++ compiler is needed. */
struct ManagerInfo {
    void *type_vtable, *spare;
    char name[32];
    uint32_t locator[6];
};
static struct ManagerInfo manager_info = {
    .name = ".?AVIClientAppManagerMap@@",
    .locator = {1, 0, 0, 0, 0, offsetof(struct ManagerInfo, locator)},
};
#define MANAGER_RTTI manager_info.locator
#define SIZE_MARKER ".byte 0x41, 0xb8\n.long 0xf8\n"
#else
static struct { void *unused; const char *name; } manager_info = {0, "20IClientAppManagerMap"};
#define MANAGER_RTTI &manager_info
#define SIZE_MARKER ".byte 0xba\n.long 0xf8\n"
#endif
static uint32_t state = 4, mutations = 0;

__attribute__((used, noinline)) static uint32_t install_body(void *self, uint32_t app, uint32_t folder, bool flag) {
    if (app != 42 || folder != 0 || flag) abort();
    mutations++; state = 4; return 0;
}
__attribute__((used, noinline)) static uint32_t remove_body(void *self, uint32_t app) {
    if (app != 42) abort();
    mutations++; state = 1; return 0;
}
__attribute__((used, noinline)) static uint32_t state_body(void *self, uint32_t app) { return app == 42 ? state : 1; }

/* Include the same wire signature markers as the client proxy, behind a jump. */
#ifdef FIXTURE_NEW_PROXY
#define INSTALL_IDS ".long 0xdd9af161\n.long 0xdfbf8d85\n"
#define REMOVE_IDS ".long 0x4505e2d4\n.long 0x4730280f\n"
#define STATE_IDS ".long 0xeaf26188\n.long 0xed101990\n"
#define FOLDER_IDS ".long 0x166ebaa1\n.long 0x18c74498\n"
#define UPDATE_IDS ".long 0x27b7a577\n.long 0x29ea87d5\n"
#define QUEUE_IDS ".long 0x9bd943ae\n.long 0x9e22ca68\n"
#else
#define INSTALL_IDS ".long 0xdd9af16a\n.long 0xdfc1c497\n"
#define REMOVE_IDS ".long 0x4505e2cd\n.long 0x47325f11\n"
#define STATE_IDS ".long 0xeaf261b3\n.long 0xed1250c4\n"
#define FOLDER_IDS ".long 0x166ebab8\n.long 0x18c97bb8\n"
#define UPDATE_IDS ".long 0x27b7a56e\n.long 0x29ecbed5\n"
#define QUEUE_IDS ".long 0x9bd943b1\n.long 0x9e250174\n"
#endif
__attribute__((naked, noinline)) static uint32_t install_map(void *self, uint32_t app, uint32_t folder, bool flag) {
    __asm__("jmp install_body\n" INSTALL_IDS);
}
__attribute__((naked, noinline)) static uint32_t remove_map(void *self, uint32_t app) {
    __asm__("jmp remove_body\n" REMOVE_IDS);
}
__attribute__((naked, noinline)) static uint32_t state_map(void *self, uint32_t app) {
    __asm__("jmp state_body\n" STATE_IDS);
}
__attribute__((naked, noinline)) static uint32_t folder_map(void *self, uint32_t app) {
    __asm__("xor %eax, %eax\nret\n" FOLDER_IDS);
}
__attribute__((used, noinline)) static bool update_body(void *self, uint32_t app, uint64_t *out) {
    memset(out, 0, 248); out[16] = 450; out[17] = 1000; return app == 42;
}
__attribute__((naked, noinline)) static bool update_map(void *self, uint32_t app, uint64_t *out) {
    __asm__("jmp update_body\n" UPDATE_IDS SIZE_MARKER);
}
__attribute__((used, noinline)) static bool queue_body(void *self, uint32_t app, uint32_t placement) {
    if (app != 42 || (placement != 1 && placement != 6)) abort();
    mutations++; state = 2 | (placement == 6 ? 512 : (256 | 1024)); return true;
}
__attribute__((naked, noinline)) static bool queue_map(void *self, uint32_t app, uint32_t placement) {
    __asm__("jmp queue_body\n" QUEUE_IDS);
}
static void *get_manager(void *self, int user, int pipe, const char *version) {
    if (pipe != 7 || user != 8 || strcmp(version, "CLIENTAPPMANAGER_INTERFACE_VERSION001")) abort();
    return &manager;
}
void fixture_state(uint32_t value) { state = value; }
void fixture_incompatible(void) {
#ifdef _WIN32
    manager_info.name[0] = '!';
#else
    manager_info.name = "21ChangedAppManagerMap";
#endif
}
uint32_t fixture_mutations(void) { return mutations; }
void *CreateInterface(const char *version, int *result) {
    if (strcmp(version, "CLIENTENGINE_INTERFACE_VERSION005")) return CreateOwnershipInterface(version, result);
    engine_table[43] = get_manager; engine.vtable = engine_table;
    manager_table[0] = MANAGER_RTTI;
    manager_table[1] = install_map; manager_table[2] = remove_map; manager_table[5] = state_map;
    manager_table[83] = folder_map;
    manager_table[21] = update_map; manager_table[57] = queue_map;
    manager.vtable = &manager_table[1];
    return &engine;
}
