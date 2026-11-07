/* A real binary at the tested path: coreutils may rewrite symlinked argv[0]. */
#include <unistd.h>
int main(void) { for (;;) pause(); }
