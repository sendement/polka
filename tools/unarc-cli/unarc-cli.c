/* unarc-cli: unpacks a FreeArc archive through the unarc.dll a repack
 * ships (with the compressors its arc.ini names), for Polka under Wine.
 *
 *   unarc-cli.exe <archive> <dest> [password]
 *
 * Run from the folder that holds unarc.dll, arc.ini and the cls-* files.
 * Prints "total N", "read N", "write N" (MiB), "file NAME", "error N TEXT"
 * and "result N" lines, flushed as they come.
 */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef int (__stdcall *cb_t)(char *what, int int1, int int2, char *str);
typedef int (__cdecl *extract_t)(cb_t cb, ...);

static const char *password;

/* The CLS plugins (cls-*.dll) talk to their 64-bit halves through named
 * events and file mappings; the halves make them "Global\..." and the
 * plugins look for the bare name, which Wine keeps apart. Their imports are
 * pointed at these: the bare name first, then the Global one. And their
 * message boxes print instead of showing a window. */
static HANDLE WINAPI open_mapping(DWORD access, BOOL inherit, LPCSTR name)
{
    HANDLE h = OpenFileMappingA(access, inherit, name);
    if (!h && name && strncmp(name, "Global\\", 7)) {
        char g[512];
        snprintf(g, sizeof g, "Global\\%s", name);
        h = OpenFileMappingA(access, inherit, g);
    }
    return h;
}

static HANDLE WINAPI open_event(DWORD access, BOOL inherit, LPCSTR name)
{
    HANDLE h = OpenEventA(access, inherit, name);
    if (!h && name && strncmp(name, "Global\\", 7)) {
        char g[512];
        snprintf(g, sizeof g, "Global\\%s", name);
        h = OpenEventA(access, inherit, g);
    }
    return h;
}

static int WINAPI message_box(HWND w, LPCSTR text, LPCSTR caption, UINT type)
{
    (void)w; (void)type;
    printf("error -3 %s: %s\n", caption ? caption : "", text ? text : "");
    fflush(stdout);
    return IDOK;
}

/* Point module m's imports of `from` at `to`. */
static void patch(HMODULE m, void *from, void *to)
{
    BYTE *base = (BYTE *)m;
    IMAGE_NT_HEADERS *nt = (IMAGE_NT_HEADERS *)(base + ((IMAGE_DOS_HEADER *)base)->e_lfanew);
    IMAGE_DATA_DIRECTORY dir = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
    if (!dir.VirtualAddress) return;
    for (IMAGE_IMPORT_DESCRIPTOR *d = (IMAGE_IMPORT_DESCRIPTOR *)(base + dir.VirtualAddress); d->Name; d++) {
        for (IMAGE_THUNK_DATA *t = (IMAGE_THUNK_DATA *)(base + d->FirstThunk); t->u1.Function; t++) {
            if ((void *)t->u1.Function == from) {
                DWORD old;
                VirtualProtect(&t->u1.Function, sizeof(void *), PAGE_READWRITE, &old);
                t->u1.Function = (ULONG_PTR)to;
                VirtualProtect(&t->u1.Function, sizeof(void *), old, &old);
            }
        }
    }
}

/* Every cls-*.dll here loaded now (unarc gets the same ones) and patched. */
static void prepare_plugins(void)
{
    HMODULE k = GetModuleHandleA("kernel32.dll"), u = LoadLibraryA("user32.dll");
    void *ofm = (void *)GetProcAddress(k, "OpenFileMappingA"), *oev = (void *)GetProcAddress(k, "OpenEventA");
    void *mb = u ? (void *)GetProcAddress(u, "MessageBoxA") : NULL;
    WIN32_FIND_DATAA f;
    HANDLE s = FindFirstFileA("cls-*.dll", &f);
    if (s == INVALID_HANDLE_VALUE) return;
    do {
        HMODULE m = LoadLibraryA(f.cFileName);
        if (!m) continue;
        patch(m, ofm, (void *)open_mapping);
        patch(m, oev, (void *)open_event);
        if (mb) patch(m, mb, (void *)message_box);
    } while (FindNextFileA(s, &f));
    FindClose(s);
}

static int __stdcall callback(char *what, int int1, int int2, char *str)
{
    if (!what) return 0;
    if (!strcmp(what, "total")) printf("total %d\n", int1);
    else if (!strcmp(what, "total_files")) printf("files %d\n", int1);
    else if (!strcmp(what, "read")) printf("read %d\n", int1);
    else if (!strcmp(what, "write")) printf("write %d\n", int1);
    else if (!strcmp(what, "filename")) printf("file %s\n", str ? str : "");
    else if (!strcmp(what, "error")) {
        /* (unarc goes on after some errors, round and round: stop.) */
        static int errors;
        printf("error %d %s\n", int1, str ? str : "");
        fflush(stdout);
        if (++errors > 10) ExitProcess(4);
    }
    else if (!strcmp(what, "overwrite?")) { fflush(stdout); return 'y'; }
    else if (!strcmp(what, "password?")) {
        if (password && str && int1 > 0) { strncpy(str, password, int1 - 1); str[int1 - 1] = 0; return 'y'; }
        printf("error -1 password\n");
        fflush(stdout);
        return 'q';
    }
    fflush(stdout);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc < 3) {
        fprintf(stderr, "usage: unarc-cli <archive> <dest> [password]\n");
        return 2;
    }
    password = argc > 3 ? argv[3] : NULL;
    prepare_plugins();
    HMODULE h = LoadLibraryA("unarc.dll");
    if (!h) { printf("error -2 no unarc.dll\n"); return 3; }
    extract_t extract = (extract_t)GetProcAddress(h, "FreeArcExtract");
    if (!extract) { printf("error -2 no FreeArcExtract\n"); return 3; }
    char dest[MAX_PATH + 8];
    snprintf(dest, sizeof dest, "-dp%s", argv[2]);
    int r = extract(callback, "x", "-o+", "-cfgarc.ini", "-ld1000m", dest, "--", argv[1], NULL, NULL, NULL, NULL);
    printf("result %d\n", r);
    fflush(stdout);
    return r == 0 ? 0 : 1;
}
