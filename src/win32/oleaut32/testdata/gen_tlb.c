/* Writes the type library fixtures of rust-dos's OLEAUT32 tests with
   Windows' own writers: CreateTypeLib (SLTG) or CreateTypeLib2 (MSFT).
   gen_tlb sltg|msft OUT.tlb */
#define COBJMACROS
#include <windows.h>
#include <oleauto.h>
#include <stdio.h>

#define CHECK(x) do { HRESULT hr_ = (x); if (FAILED(hr_)) { \
    printf("%s failed: %08lx (line %d)\n", #x, hr_, __LINE__); exit(1); } } while (0)

static const GUID LIBID = {0x11111111, 0x2222, 0x3333, {0x44,0x44,0x55,0x55,0x55,0x55,0x55,0x55}};
static const GUID IID_IShape = {0x11111111, 0x2222, 0x3333, {0x44,0x44,0x00,0x00,0x00,0x00,0x00,0x01}};
static const GUID IID_DEvents = {0x11111111, 0x2222, 0x3333, {0x44,0x44,0x00,0x00,0x00,0x00,0x00,0x02}};
static const GUID IID_IPlain = {0x11111111, 0x2222, 0x3333, {0x44,0x44,0x00,0x00,0x00,0x00,0x00,0x03}};
static const GUID CLSID_Shape = {0x11111111, 0x2222, 0x3333, {0x44,0x44,0x00,0x00,0x00,0x00,0x00,0x04}};
static const GUID GUID_Colour = {0x11111111, 0x2222, 0x3333, {0x44,0x44,0x00,0x00,0x00,0x00,0x00,0x05}};

/* Which writer: the old one (ICreateTypeLib) or the new one. */
static int sltg;
static ICreateTypeLib *lib;

static ICreateTypeInfo *new_type(const wchar_t *name, TYPEKIND kind, const GUID *guid,
                                 const wchar_t *doc)
{
    ICreateTypeInfo *ti;
    CHECK(ICreateTypeLib_CreateTypeInfo(lib, (LPOLESTR)name, kind, &ti));
    if (guid) CHECK(ICreateTypeInfo_SetGuid(ti, guid));
    if (doc) CHECK(ICreateTypeInfo_SetDocString(ti, (LPOLESTR)doc));
    return ti;
}

static void add_func(ICreateTypeInfo *ti, UINT index, MEMBERID id, FUNCKIND fk, INVOKEKIND ik,
                     VARTYPE ret, ELEMDESC *params, SHORT n, SHORT opt, const wchar_t **names,
                     const wchar_t *doc)
{
    FUNCDESC fd;
    memset(&fd, 0, sizeof fd);
    fd.memid = id;
    fd.funckind = fk;
    fd.invkind = ik;
    fd.callconv = CC_STDCALL;
    fd.cParams = n;
    fd.cParamsOpt = opt;
    fd.lprgelemdescParam = params;
    fd.elemdescFunc.tdesc.vt = ret;
    CHECK(ICreateTypeInfo_AddFuncDesc(ti, index, &fd));
    /* A propput's last parameter has no name of its own. */
    CHECK(ICreateTypeInfo_SetFuncAndParamNames(ti, index, (LPOLESTR *)names,
                                               ik == INVOKE_PROPERTYPUT ? n : n + 1));
    if (doc) CHECK(ICreateTypeInfo_SetFuncDocString(ti, index, (LPOLESTR)doc));
}

static void add_var(ICreateTypeInfo *ti, UINT index, MEMBERID id, VARKIND kind, VARTYPE vt,
                    VARIANT *value, ULONG inst, const wchar_t *name)
{
    VARDESC vd;
    memset(&vd, 0, sizeof vd);
    vd.memid = id;
    vd.varkind = kind;
    vd.elemdescVar.tdesc.vt = vt;
    if (kind == VAR_CONST) vd.lpvarValue = value;
    else vd.oInst = inst;
    CHECK(ICreateTypeInfo_AddVarDesc(ti, index, &vd));
    CHECK(ICreateTypeInfo_SetVarName(ti, index, (LPOLESTR)name));
}

static void param(ELEMDESC *e, VARTYPE vt, TYPEDESC *pointee, USHORT flags)
{
    memset(e, 0, sizeof *e);
    e->tdesc.vt = vt;
    if (vt == VT_PTR) e->tdesc.lptdesc = pointee;
    e->paramdesc.wParamFlags = flags;
}

int wmain(int argc, wchar_t **argv)
{
    ICreateTypeInfo *ti;
    ITypeLib *stdole;
    ITypeInfo *dispatch, *unknown, *made;
    HREFTYPE href;
    ELEMDESC p[2];
    TYPEDESC long_t, bstr_t, user_t, user2_t;
    VARIANT v;
    PARAMDESCEX dflt;

    if (argc != 3) return 2;
    sltg = !wcscmp(argv[1], L"sltg");
    CoInitialize(NULL);
    if (sltg)
        CHECK(CreateTypeLib(SYS_WIN32, argv[2], &lib));
    else
        CHECK(CreateTypeLib2(SYS_WIN32, argv[2], (ICreateTypeLib2 **)&lib));
    CHECK(ICreateTypeLib_SetGuid(lib, &LIBID));
    CHECK(ICreateTypeLib_SetName(lib, L"RdTest"));
    CHECK(ICreateTypeLib_SetVersion(lib, 1, 2));
    CHECK(ICreateTypeLib_SetLcid(lib, 0));
    CHECK(ICreateTypeLib_SetDocString(lib, L"rust-dos type library test"));
    CHECK(ICreateTypeLib_SetHelpFileName(lib, L"rdtest.hlp"));
    CHECK(ICreateTypeLib_SetHelpContext(lib, 7));

    if (sltg) CHECK(LoadTypeLibEx(L"stdole32.tlb", REGKIND_NONE, &stdole));
    else CHECK(LoadRegTypeLib(&IID_StdOle, 2, 0, 0, &stdole));
    CHECK(ITypeLib_GetTypeInfoOfGuid(stdole, &IID_IDispatch, &dispatch));
    CHECK(ITypeLib_GetTypeInfoOfGuid(stdole, &IID_IUnknown, &unknown));

    long_t.vt = VT_I4;
    bstr_t.vt = VT_BSTR;

    /* enum Colour { Red = 0, Green = 1, Blue = 5 } */
    ti = new_type(L"Colour", TKIND_ENUM, &GUID_Colour, L"Colours");
    V_VT(&v) = VT_I4;
    V_I4(&v) = 0; add_var(ti, 0, MEMBERID_NIL, VAR_CONST, VT_I4, &v, 0, L"Red");
    V_I4(&v) = 1; add_var(ti, 1, MEMBERID_NIL, VAR_CONST, VT_I4, &v, 0, L"Green");
    V_I4(&v) = 5; add_var(ti, 2, MEMBERID_NIL, VAR_CONST, VT_I4, &v, 0, L"Blue");
    CHECK(ICreateTypeInfo_LayOut(ti));
    ICreateTypeInfo_Release(ti);

    /* struct Point { long x; long y; } */
    ti = new_type(L"Point", TKIND_RECORD, NULL, NULL);
    add_var(ti, 0, MEMBERID_NIL, VAR_PERINSTANCE, VT_I4, NULL, 0, L"x");
    add_var(ti, 1, MEMBERID_NIL, VAR_PERINSTANCE, VT_I4, NULL, 4, L"y");
    CHECK(ICreateTypeInfo_LayOut(ti));
    CHECK(ICreateTypeInfo_QueryInterface(ti, &IID_ITypeInfo, (void **)&made));
    ICreateTypeInfo_Release(ti);

    /* typedef long Handle; */
    ti = new_type(L"Handle", TKIND_ALIAS, NULL, NULL);
    CHECK(ICreateTypeInfo_SetTypeDescAlias(ti, &long_t));
    CHECK(ICreateTypeInfo_LayOut(ti));
    ICreateTypeInfo_Release(ti);

    /* [dual, oleautomation] interface IShape : IDispatch */
    ti = new_type(L"IShape", TKIND_INTERFACE, &IID_IShape, L"A shape");
    /* Windows' SLTG writer fails LayOut (E_NOTIMPL) for an interface
       with a base, so the SLTG fixture's have none. */
    CHECK(ICreateTypeInfo_SetTypeFlags(ti, (sltg ? 0 : TYPEFLAG_FDUAL) | TYPEFLAG_FOLEAUTOMATION));
    if (!sltg) {
        CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, dispatch, &href));
        CHECK(ICreateTypeInfo_AddImplType(ti, 0, href));
    }
    {
        static const wchar_t *get_names[] = {L"Width", L"value"};
        static const wchar_t *put_names[] = {L"Width"};
        static const wchar_t *move_names[] = {L"Move", L"dx", L"dy"};
        static const wchar_t *name_names[] = {L"Name", L"name"};
        param(&p[0], VT_PTR, &long_t, PARAMFLAG_FOUT | PARAMFLAG_FRETVAL);
        add_func(ti, 0, 1, FUNC_PUREVIRTUAL, INVOKE_PROPERTYGET, VT_HRESULT, p, 1, 0,
                 get_names, L"The width");
        param(&p[0], VT_I4, NULL, PARAMFLAG_FIN);
        add_func(ti, 1, 1, FUNC_PUREVIRTUAL, INVOKE_PROPERTYPUT, VT_HRESULT, p, 1, 0,
                 put_names, NULL);
        param(&p[0], VT_I4, NULL, PARAMFLAG_FIN);
        param(&p[1], VT_I4, NULL, PARAMFLAG_FIN | PARAMFLAG_FOPT | PARAMFLAG_FHASDEFAULT);
        memset(&dflt, 0, sizeof dflt);
        dflt.cBytes = sizeof dflt;
        V_VT(&dflt.varDefaultValue) = VT_I4;
        V_I4(&dflt.varDefaultValue) = 3;
        p[1].paramdesc.pparamdescex = &dflt;
        add_func(ti, 2, 2, FUNC_PUREVIRTUAL, INVOKE_FUNC, VT_HRESULT, p, 2, 0, move_names,
                 L"Moves it");
        param(&p[0], VT_PTR, &bstr_t, PARAMFLAG_FOUT | PARAMFLAG_FRETVAL);
        add_func(ti, 3, 3, FUNC_PUREVIRTUAL, INVOKE_FUNC, VT_HRESULT, p, 1, 0, name_names,
                 NULL);
    }
    CHECK(ICreateTypeInfo_LayOut(ti));
    {
        ITypeInfo *shape;
        CHECK(ICreateTypeInfo_QueryInterface(ti, &IID_ITypeInfo, (void **)&shape));
        ICreateTypeInfo_Release(ti);

        /* dispinterface DShapeEvents { methods: [id(1)] void Moved(long x, long y); } */
        ti = new_type(L"DShapeEvents", TKIND_DISPATCH, &IID_DEvents, NULL);
        if (!sltg) {
            CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, dispatch, &href));
            CHECK(ICreateTypeInfo_AddImplType(ti, 0, href));
        }
        {
            static const wchar_t *moved_names[] = {L"Moved", L"x", L"y"};
            param(&p[0], VT_I4, NULL, PARAMFLAG_FIN);
            param(&p[1], VT_I4, NULL, PARAMFLAG_FIN);
            add_func(ti, 0, 1, FUNC_DISPATCH, INVOKE_FUNC, VT_VOID, p, 2, 0, moved_names, NULL);
        }
        CHECK(ICreateTypeInfo_LayOut(ti));
        {
            ITypeInfo *events;
            CHECK(ICreateTypeInfo_QueryInterface(ti, &IID_ITypeInfo, (void **)&events));
            ICreateTypeInfo_Release(ti);

            /* [oleautomation] interface IPlain : IUnknown
               { HRESULT Draw([in] IShape *s, [in] Point *p); } */
            ti = new_type(L"IPlain", TKIND_INTERFACE, &IID_IPlain, NULL);
            CHECK(ICreateTypeInfo_SetTypeFlags(ti, TYPEFLAG_FOLEAUTOMATION));
            if (!sltg) {
                CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, unknown, &href));
                CHECK(ICreateTypeInfo_AddImplType(ti, 0, href));
            }
            {
                static const wchar_t *draw_names[] = {L"Draw", L"s", L"p"};
                CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, shape, &href));
                user_t.vt = VT_USERDEFINED;
                user_t.hreftype = href;
                CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, made, &href));
                user2_t.vt = VT_USERDEFINED;
                user2_t.hreftype = href;
                param(&p[0], VT_PTR, &user_t, PARAMFLAG_FIN);
                param(&p[1], VT_PTR, &user2_t, PARAMFLAG_FIN);
                add_func(ti, 0, MEMBERID_NIL, FUNC_PUREVIRTUAL, INVOKE_FUNC, VT_HRESULT, p, 2, 0,
                         draw_names, NULL);
            }
            CHECK(ICreateTypeInfo_LayOut(ti));
            ICreateTypeInfo_Release(ti);

            /* coclass Shape { [default] interface IShape;
                               [default, source] dispinterface DShapeEvents; } */
            ti = new_type(L"Shape", TKIND_COCLASS, &CLSID_Shape, L"The shape class");
            CHECK(ICreateTypeInfo_SetTypeFlags(ti, TYPEFLAG_FCANCREATE));
            CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, shape, &href));
            CHECK(ICreateTypeInfo_AddImplType(ti, 0, href));
            CHECK(ICreateTypeInfo_SetImplTypeFlags(ti, 0, IMPLTYPEFLAG_FDEFAULT));
            CHECK(ICreateTypeInfo_AddRefTypeInfo(ti, events, &href));
            CHECK(ICreateTypeInfo_AddImplType(ti, 1, href));
            CHECK(ICreateTypeInfo_SetImplTypeFlags(ti, 1,
                                                   IMPLTYPEFLAG_FDEFAULT | IMPLTYPEFLAG_FSOURCE));
            CHECK(ICreateTypeInfo_LayOut(ti));
            ICreateTypeInfo_Release(ti);
        }
    }
    CHECK(ICreateTypeLib_SaveAllChanges(lib));
    ICreateTypeLib_Release(lib);
    printf("wrote %ls\n", argv[2]);
    return 0;
}
