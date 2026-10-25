/* Included before each of shell32's tests. Wine builds them into one
 * program, which imports SHELL32 for the functions some of them call;
 * the tests that only take SHELL32's functions with GetProcAddress
 * (shellpath.c, string.c) find the DLL with GetModuleHandleA, so their
 * programs here import a function of it too. The import is named by
 * its symbol, so no header comes before the test's own. */
extern void *shell32_import __asm__("__imp__SHGetMalloc@4");
static void *const *const shell32_imported __attribute__((used)) = &shell32_import;
