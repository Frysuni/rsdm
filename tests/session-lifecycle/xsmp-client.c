#define _POSIX_C_SOURCE 200809L

#include <X11/SM/SMlib.h>
#include <X11/ICE/ICElib.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

struct client {
    const char *mode;
    const char *id;
    const char *runtime;
    SmcConn connection;
    int saved;
    int cancelled;
    int saving;
};

static struct client *active_client;

static void marker(struct client *client, const char *event) {
    char path[1024];
    snprintf(path, sizeof(path), "%s/xsmp-%s-%s", client->runtime, client->id, event);
    FILE *file = fopen(path, "w");
    if (!file) exit(3);
    fputs(event, file);
    fclose(file);
}

static void display_alive(struct client *client) {
    char path[1024];
    snprintf(path, sizeof(path), "%s/display-alive", client->runtime);
    if (access(path, F_OK) != 0) exit(4);
}

static void pause_millis(long millis) {
    struct timespec delay = { .tv_sec = millis / 1000, .tv_nsec = (millis % 1000) * 1000000 };
    while (nanosleep(&delay, &delay) != 0 && errno == EINTR) {}
}

static void phase2(SmcConn connection, SmPointer data) {
    struct client *client = data;
    display_alive(client);
    char path[1024];
    snprintf(path, sizeof(path), "%s/xsmp-first-first-done", client->runtime);
    if (access(path, F_OK) != 0) exit(5);
    marker(client, "phase2");
    marker(client, "saved");
    client->saved = 1;
    client->saving = 0;
    SmcSaveYourselfDone(connection, True);
}

static void interact(SmcConn connection, SmPointer data) {
    struct client *client = data;
    display_alive(client);
    marker(client, "interacted");
    SmcInteractDone(connection, True);
}

static void save(SmcConn connection, SmPointer data, int kind, Bool shutdown, int style, Bool fast) {
    struct client *client = data;
    (void) fast;
    if (!shutdown) {
        SmcSaveYourselfDone(connection, True);
        return;
    }
    if (kind != SmSaveBoth || style != SmInteractStyleAny) exit(6);
    display_alive(client);
    marker(client, "saving");
    client->saving = 1;
    if (strcmp(client->mode, "phase2") == 0) {
        if (!SmcRequestSaveYourselfPhase2(connection, phase2, client)) exit(7);
    } else if (strcmp(client->mode, "cancel") == 0 && !client->cancelled) {
        if (!SmcInteractRequest(connection, SmDialogNormal, interact, client)) exit(8);
    } else if (strcmp(client->mode, "failed") == 0 && !client->cancelled) {
        client->saving = 0;
        SmcSaveYourselfDone(connection, False);
    } else if (strcmp(client->mode, "silent") != 0) {
        if (strcmp(client->mode, "slow-first") == 0) pause_millis(400);
        marker(client, "first-done");
        marker(client, "saved");
        client->saved = 1;
        client->saving = 0;
        SmcSaveYourselfDone(connection, True);
    }
}

static void die(SmcConn connection, SmPointer data) {
    struct client *client = data;
    display_alive(client);
    if (!client->saved) exit(9);
    marker(client, "die");
    if (strcmp(client->mode, "stay") == 0) return;
    SmcCloseConnection(connection, 0, NULL);
    exit(0);
}

static void save_complete(SmcConn connection, SmPointer data) {
    (void) connection;
    struct client *client = data;
    marker(client, "ready");
    if (client->cancelled) marker(client, "resumed");
}

static void shutdown_cancelled(SmcConn connection, SmPointer data) {
    struct client *client = data;
    client->cancelled = 1;
    marker(client, "cancelled");
    if (client->saving) {
        client->saving = 0;
        SmcSaveYourselfDone(connection, True);
    }
}

static void io_error(IceConn connection) {
    (void) connection;
    if (active_client && strcmp(active_client->mode, "rejected") == 0) {
        marker(active_client, "rejected");
        exit(0);
    }
    exit(10);
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    static struct client client;
    client = (struct client) { .mode = argv[1], .id = argv[2], .runtime = getenv("XDG_RUNTIME_DIR") };
    active_client = &client;
    if (!client.runtime) return 2;
    signal(SIGTERM, SIG_IGN);
    IceSetIOErrorHandler(io_error);
    SmcCallbacks callbacks = {
        .save_yourself = { save, &client },
        .die = { die, &client },
        .save_complete = { save_complete, &client },
        .shutdown_cancelled = { shutdown_cancelled, &client },
    };
    char error[256] = {0};
    char *id = NULL;
    unsigned long mask = SmcSaveYourselfProcMask | SmcDieProcMask |
        SmcSaveCompleteProcMask | SmcShutdownCancelledProcMask;
    client.connection = SmcOpenConnection(NULL, NULL, 1, 0, mask, &callbacks, NULL, &id, sizeof(error), error);
    free(id);
    if (!client.connection) {
        fprintf(stderr, "XSMP: %s\n", error);
        if (strcmp(client.mode, "rejected") == 0) { marker(&client, "rejected"); return 0; }
        return 11;
    }
    if (strcmp(client.mode, "rejected") == 0) return 13;
    char claimed_pid[] = "1";
    SmPropValue value = { .length = 1, .value = claimed_pid };
    SmProp property = { .name = SmProcessID, .type = SmARRAY8, .num_vals = 1, .vals = &value };
    SmProp *properties[] = { &property };
    SmcSetProperties(client.connection, 1, properties);
    IceConn ice = SmcGetIceConnection(client.connection);
    while (IceProcessMessages(ice, NULL, NULL) == IceProcessMessagesSuccess) {}
    return 12;
}
