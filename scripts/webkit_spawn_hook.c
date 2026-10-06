#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <spawn.h>
#include <dlfcn.h>
#include <limits.h>

static const char* redirect_path(const char* path, char* buf, size_t buf_len) {
    if (!path) return path;
    const char* appdir = getenv("APPDIR");
    if (!appdir) return path;

    // 1. Direct match inside APPDIR if path starts with /usr/
    if (strncmp(path, "/usr/", 5) == 0) {
        snprintf(buf, buf_len, "%s%s", appdir, path);
        if (access(buf, F_OK) == 0) {
            return buf;
        }
    }

    // 2. Identify WebKit child processes and helpers
    const char* base = strrchr(path, '/');
    const char* fname = base ? (base + 1) : path;
    if (strstr(fname, "WebKitNetworkProcess") ||
        strstr(fname, "WebKitWebProcess") ||
        strstr(fname, "WebKitGPUProcess") ||
        strstr(fname, "jsc") ||
        strstr(fname, "libwebkit2gtkinjectedbundle.so") ||
        strstr(path, "webkit2gtk-4.1")) {

        // Check common relative paths in APPDIR
        snprintf(buf, buf_len, "%s/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/lib/aarch64-linux-gnu/webkit2gtk-4.1/injected-bundle/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/lib/aarch64-linux-gnu/webkit2gtk-4.1/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/lib/webkit2gtk-4.1/injected-bundle/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/lib/webkit2gtk-4.1/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/libexec/webkit2gtk-4.1/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;

        snprintf(buf, buf_len, "%s/usr/lib/%s", appdir, fname);
        if (access(buf, F_OK) == 0) return buf;
    }
    return path;
}

void *dlopen(const char *filename, int flag) {
    static void *(*real_dlopen)(const char *, int) = NULL;
    if (!real_dlopen) real_dlopen = dlsym(RTLD_NEXT, "dlopen");
    char redirected[PATH_MAX];
    const char* target = redirect_path(filename, redirected, sizeof(redirected));
    return real_dlopen(target, flag);
}

int execve(const char *pathname, char *const argv[], char *const envp[]) {
    static int (*real_execve)(const char *, char *const [], char *const []) = NULL;
    if (!real_execve) real_execve = dlsym(RTLD_NEXT, "execve");
    char redirected[PATH_MAX];
    const char* target = redirect_path(pathname, redirected, sizeof(redirected));
    return real_execve(target, argv, envp);
}

int execv(const char *pathname, char *const argv[]) {
    static int (*real_execv)(const char *, char *const []) = NULL;
    if (!real_execv) real_execv = dlsym(RTLD_NEXT, "execv");
    char redirected[PATH_MAX];
    const char* target = redirect_path(pathname, redirected, sizeof(redirected));
    return real_execv(target, argv);
}

int execvp(const char *file, char *const argv[]) {
    static int (*real_execvp)(const char *, char *const []) = NULL;
    if (!real_execvp) real_execvp = dlsym(RTLD_NEXT, "execvp");
    char redirected[PATH_MAX];
    const char* target = redirect_path(file, redirected, sizeof(redirected));
    return real_execvp(target, argv);
}

int execvpe(const char *file, char *const argv[], char *const envp[]) {
    static int (*real_execvpe)(const char *, char *const [], char *const []) = NULL;
    if (!real_execvpe) real_execvpe = dlsym(RTLD_NEXT, "execvpe");
    char redirected[PATH_MAX];
    const char* target = redirect_path(file, redirected, sizeof(redirected));
    return real_execvpe(target, argv, envp);
}

int posix_spawn(pid_t *pid, const char *path,
                const posix_spawn_file_actions_t *file_actions,
                const posix_spawnattr_t *attrp,
                char *const argv[], char *const envp[]) {
    static int (*real_posix_spawn)(pid_t *, const char *,
                                   const posix_spawn_file_actions_t *,
                                   const posix_spawnattr_t *,
                                   char *const [], char *const []) = NULL;
    if (!real_posix_spawn) real_posix_spawn = dlsym(RTLD_NEXT, "posix_spawn");
    char redirected[PATH_MAX];
    const char* target = redirect_path(path, redirected, sizeof(redirected));
    return real_posix_spawn(pid, target, file_actions, attrp, argv, envp);
}

int posix_spawnp(pid_t *pid, const char *file,
                 const posix_spawn_file_actions_t *file_actions,
                 const posix_spawnattr_t *attrp,
                 char *const argv[], char *const envp[]) {
    static int (*real_posix_spawnp)(pid_t *, const char *,
                                    const posix_spawn_file_actions_t *,
                                    const posix_spawnattr_t *,
                                    char *const [], char *const []) = NULL;
    if (!real_posix_spawnp) real_posix_spawnp = dlsym(RTLD_NEXT, "posix_spawnp");
    char redirected[PATH_MAX];
    const char* target = redirect_path(file, redirected, sizeof(redirected));
    return real_posix_spawnp(pid, target, file_actions, attrp, argv, envp);
}
