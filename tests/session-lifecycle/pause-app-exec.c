#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Pause after exec so the test can replace the binary before its main runs. */
__attribute__((constructor)) static void pause_app_exec(int argc, char **argv) {
    if (!getenv("RSDM_PAUSE_APP_EXEC")) return;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "app-exec") != 0) continue;
        unsetenv("LD_PRELOAD");
        unsetenv("RSDM_PAUSE_APP_EXEC");

        FILE *file = fopen("/run/user/1000/app-exec-paused", "w");
        if (!file) _exit(1);
        if (fprintf(file, "%d\n", getpid()) < 0 || fclose(file) != 0) _exit(1);
        raise(SIGSTOP);
        return;
    }
}
