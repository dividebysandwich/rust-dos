/* Included before shell32's shlfileop.c. The test declares
 * Shell_MergeMenus itself, returning an HRESULT, where MinGW's shlobj.h
 * has it return a UINT; its calls go to a function of the test's own
 * type that calls the DLL's. */
#include <windows.h>
#include <shlobj.h>

static HRESULT WINAPI wine_Shell_MergeMenus(HMENU dst, HMENU src, UINT insert,
                                            UINT first, UINT last, ULONG flags)
{
    return Shell_MergeMenus(dst, src, insert, first, last, flags);
}

#define Shell_MergeMenus wine_Shell_MergeMenus
