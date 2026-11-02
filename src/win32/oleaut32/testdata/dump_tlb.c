/* Prints what Windows' OLEAUT32 reads from a type library, for the
   expectations of rust-dos's tests. dump_tlb FILE */
#define COBJMACROS
#include <windows.h>
#include <oleauto.h>
#include <stdio.h>

static void td(ITypeInfo *ti, TYPEDESC *t)
{
    switch (t->vt) {
    case VT_PTR: printf("ptr("); td(ti, t->lptdesc); printf(")"); break;
    case VT_SAFEARRAY: printf("safearray("); td(ti, t->lptdesc); printf(")"); break;
    case VT_CARRAY:
        printf("carray[%d](", t->lpadesc->cDims); td(ti, &t->lpadesc->tdescElem); printf(")");
        break;
    case VT_USERDEFINED: {
        ITypeInfo *r;
        BSTR name = NULL;
        printf("user(%lx", t->hreftype);
        if (SUCCEEDED(ITypeInfo_GetRefTypeInfo(ti, t->hreftype, &r))) {
            ITypeInfo_GetDocumentation(r, MEMBERID_NIL, &name, NULL, NULL, NULL);
            printf(" %ls", name);
            ITypeInfo_Release(r);
        }
        printf(")");
        break;
    }
    default: printf("%d", t->vt);
    }
}

static void str(const char *what, BSTR s)
{
    if (s) printf(" %s=\"%ls\"", what, s); else printf(" %s=null", what);
}

static void info(ITypeInfo *ti, int depth)
{
    TYPEATTR *a;
    BSTR name = NULL, doc = NULL, file = NULL;
    DWORD ctx = 0;
    UINT i;
    WCHAR g[40];
    if (FAILED(ITypeInfo_GetTypeAttr(ti, &a))) { printf("  no attr\n"); return; }
    ITypeInfo_GetDocumentation(ti, MEMBERID_NIL, &name, &doc, &ctx, &file);
    StringFromGUID2(&a->guid, g, 40);
    printf("%*stype %ls kind=%d guid=%ls flags=%x ver=%d.%d funcs=%d vars=%d impl=%d vft=%d size=%lu align=%d lcid=%lx",
           depth, "", name, a->typekind, g, a->wTypeFlags, a->wMajorVerNum, a->wMinorVerNum, a->cFuncs,
           a->cVars, a->cImplTypes, a->cbSizeVft, a->cbSizeInstance, a->cbAlignment, a->lcid);
    str("doc", doc);
    printf(" ctx=%lu", ctx);
    str("file", file);
    if (a->typekind == TKIND_ALIAS) { printf(" alias="); td(ti, &a->tdescAlias); }
    printf("\n");
    for (i = 0; i < a->cImplTypes; i++) {
        HREFTYPE h;
        INT f = -1;
        ITypeInfo *r;
        HRESULT hr = ITypeInfo_GetRefTypeOfImplType(ti, i, &h);
        ITypeInfo_GetImplTypeFlags(ti, i, &f);
        printf("%*s impl %u hr=%lx href=%lx flags=%d", depth, "", i, hr, h, f);
        if (SUCCEEDED(hr) && SUCCEEDED(ITypeInfo_GetRefTypeInfo(ti, h, &r))) {
            BSTR n = NULL;
            ITypeInfo_GetDocumentation(r, MEMBERID_NIL, &n, NULL, NULL, NULL);
            printf(" -> %ls", n);
            ITypeInfo_Release(r);
        }
        printf("\n");
    }
    {
        HREFTYPE h;
        HRESULT hr = ITypeInfo_GetRefTypeOfImplType(ti, (UINT)-1, &h);
        printf("%*s impl -1 hr=%lx", depth, "", hr);
        if (SUCCEEDED(hr)) {
            ITypeInfo *r;
            printf(" href=%lx", h);
            if (depth == 0 && SUCCEEDED(ITypeInfo_GetRefTypeInfo(ti, h, &r))) {
                printf("\n");
                info(r, 4);
                ITypeInfo_Release(r);
            }
        }
        printf("\n");
    }
    for (i = 0; i < a->cFuncs; i++) {
        FUNCDESC *f;
        BSTR names[8] = {0};
        UINT n = 0, j;
        HRESULT hr = ITypeInfo_GetFuncDesc(ti, i, &f);
        if (FAILED(hr)) { printf("%*s func %u hr=%lx\n", depth, "", i, hr); continue; }
        ITypeInfo_GetNames(ti, f->memid, names, 8, &n);
        doc = NULL;
        ITypeInfo_GetDocumentation(ti, f->memid, NULL, &doc, &ctx, NULL);
        printf("%*s func %u memid=%lx kind=%d inv=%d cc=%d params=%d opt=%d vft=%d flags=%x ret=",
               depth, "", i, f->memid, f->funckind, f->invkind, f->callconv, f->cParams,
               f->cParamsOpt, f->oVft, f->wFuncFlags);
        td(ti, &f->elemdescFunc.tdesc);
        printf(" names=%u:", n);
        for (j = 0; j < n; j++) printf(" %ls", names[j]);
        str("doc", doc);
        printf("\n");
        for (j = 0; j < (UINT)f->cParams; j++) {
            PARAMDESC *p = &f->lprgelemdescParam[j].paramdesc;
            printf("%*s  param %u flags=%x type=", depth, "", j, p->wParamFlags);
            td(ti, &f->lprgelemdescParam[j].tdesc);
            if (p->wParamFlags & PARAMFLAG_FHASDEFAULT)
                printf(" default=vt%d:%ld", V_VT(&p->pparamdescex->varDefaultValue),
                       V_I4(&p->pparamdescex->varDefaultValue));
            printf("\n");
        }
        ITypeInfo_ReleaseFuncDesc(ti, f);
    }
    for (i = 0; i < a->cVars; i++) {
        VARDESC *v;
        BSTR n = NULL;
        UINT got;
        if (FAILED(ITypeInfo_GetVarDesc(ti, i, &v))) continue;
        ITypeInfo_GetNames(ti, v->memid, &n, 1, &got);
        printf("%*s var %u memid=%lx kind=%d flags=%x name=%ls type=", depth, "", i, v->memid,
               v->varkind, v->wVarFlags, n);
        td(ti, &v->elemdescVar.tdesc);
        if (v->varkind == VAR_CONST)
            printf(" value=vt%d:%ld", V_VT(v->lpvarValue), V_I4(v->lpvarValue));
        else
            printf(" oInst=%lu", v->oInst);
        printf("\n");
        ITypeInfo_ReleaseVarDesc(ti, v);
    }
    if (!(a->typekind == TKIND_DISPATCH && !a->cImplTypes)) {
        LPOLESTR want[3] = {L"move", L"DY", L"dx"};
        MEMBERID ids[3];
        HRESULT hr = ITypeInfo_GetIDsOfNames(ti, want, 3, ids);
        printf("%*s ids(move,DY,dx) hr=%lx %lx %lx %lx\n", depth, "", hr, ids[0], ids[1], ids[2]);
    }
    ITypeInfo_ReleaseTypeAttr(ti, a);
}

int wmain(int argc, wchar_t **argv)
{
    ITypeLib *lib;
    TLIBATTR *a;
    BSTR name, doc, file;
    DWORD ctx;
    UINT i, n;
    WCHAR g[40];
    HRESULT hr;
    if (argc > 2 && !_wfreopen(argv[2], L"w", stdout)) return 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    CoInitialize(NULL);
    hr = LoadTypeLibEx(argv[1], REGKIND_NONE, &lib);
    if (FAILED(hr)) { printf("LoadTypeLibEx: %lx\n", hr); return 1; }
    ITypeLib_GetLibAttr(lib, &a);
    ITypeLib_GetDocumentation(lib, -1, &name, &doc, &ctx, &file);
    StringFromGUID2(&a->guid, g, 40);
    printf("lib %ls guid=%ls lcid=%lx syskind=%d ver=%d.%d flags=%x", name, g, a->lcid, a->syskind,
           a->wMajorVerNum, a->wMinorVerNum, a->wLibFlags);
    str("doc", doc);
    printf(" ctx=%lu", ctx);
    str("file", file);
    printf("\n");
    n = ITypeLib_GetTypeInfoCount(lib);
    for (i = 0; i < n; i++) {
        ITypeInfo *ti;
        ITypeLib_GetTypeInfo(lib, i, &ti);
        info(ti, 0);
        ITypeInfo_Release(ti);
    }
    return 0;
}
