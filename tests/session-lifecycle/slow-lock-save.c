#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* Hold only personal config writes; lock registration must remain usable. */
int fsync(int fd) {
    char descriptor[64], target[4096];
    snprintf(descriptor, sizeof(descriptor), "/proc/self/fd/%d", fd);
    ssize_t length = readlink(descriptor, target, sizeof(target) - 1);
    if (length >= 0) {
        target[length] = 0;
        const char *prefix = "/home/alice/.config/rsdm/";
        if (strncmp(target, prefix, strlen(prefix)) == 0) {
            int marker = open("/run/user/1000/rsdm-save-blocked", O_CREAT | O_WRONLY | O_CLOEXEC, 0600);
            if (marker < 0) _exit(1);
            close(marker);
            while (access("/run/user/1000/rsdm-save-continue", F_OK) != 0) usleep(10000);
        }
    }
    int (*original)(int) = dlsym(RTLD_NEXT, "fsync");
    return original(fd);
}
