/* Native Steam control test double; never connects to the real client. */
#define CreateInterface CreateOwnershipInterface
#include "steamclient.c"
#undef CreateInterface

static Object engine, manager;
static void *engine_table[44], *manager_table[84];
static struct { void *unused; const char *name; } manager_info = {0, "20IClientAppManagerMap"};
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
__attribute__((naked, noinline)) static uint32_t install_map(void *self, uint32_t app, uint32_t folder, bool flag) {
    __asm__("jmp install_body\n.long 0xdd9af16a\n.long 0xdfc1c497\n");
}
__attribute__((naked, noinline)) static uint32_t remove_map(void *self, uint32_t app) {
    __asm__("jmp remove_body\n.long 0x4505e2cd\n.long 0x47325f11\n");
}
__attribute__((naked, noinline)) static uint32_t state_map(void *self, uint32_t app) {
    __asm__("jmp state_body\n.long 0xeaf261b3\n.long 0xed1250c4\n");
}
__attribute__((naked, noinline)) static uint32_t folder_map(void *self, uint32_t app) {
    __asm__("xor %eax, %eax\nret\n.long 0x166ebab8\n.long 0x18c97bb8\n");
}
__attribute__((used, noinline)) static bool update_body(void *self, uint32_t app, uint64_t *out) {
    memset(out, 0, 248); out[16] = 450; out[17] = 1000; return app == 42;
}
__attribute__((naked, noinline)) static bool update_map(void *self, uint32_t app, uint64_t *out) {
    __asm__("jmp update_body\n.long 0x27b7a56e\n.long 0x29ecbed5\n.byte 0xba\n.long 0xf8\n");
}
__attribute__((used, noinline)) static bool queue_body(void *self, uint32_t app, uint32_t placement) {
    if (app != 42 || (placement != 1 && placement != 6)) abort();
    mutations++; state = 2 | (placement == 6 ? 512 : (256 | 1024)); return true;
}
__attribute__((naked, noinline)) static bool queue_map(void *self, uint32_t app, uint32_t placement) {
    __asm__("jmp queue_body\n.long 0x9bd943b1\n.long 0x9e250174\n");
}
static void *get_manager(void *self, int user, int pipe, const char *version) {
    if (pipe != 7 || user != 8 || strcmp(version, "CLIENTAPPMANAGER_INTERFACE_VERSION001")) abort();
    return &manager;
}
void fixture_state(uint32_t value) { state = value; }
void fixture_incompatible(void) { manager_info.name = "21ChangedAppManagerMap"; }
uint32_t fixture_mutations(void) { return mutations; }
void *CreateInterface(const char *version, int *result) {
    if (strcmp(version, "CLIENTENGINE_INTERFACE_VERSION005")) return CreateOwnershipInterface(version, result);
    engine_table[43] = get_manager; engine.vtable = engine_table;
    manager_table[0] = &manager_info;
    manager_table[1] = install_map; manager_table[2] = remove_map; manager_table[5] = state_map;
    manager_table[83] = folder_map;
    manager_table[21] = update_map; manager_table[57] = queue_map;
    manager.vtable = &manager_table[1];
    return &engine;
}
