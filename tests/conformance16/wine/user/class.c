/* Unit test suite for window classes.
 *
 * Copyright 2002 Mike McCormack
 * Copyright 2003 Alexandre Julliard
 *
 * Ported to Win16 from Wine's dlls/user32/tests/class.c: RegisterClass,
 * GetClassInfo, GetClassName, Get/SetClassLong and Get/SetClassWord,
 * UnregisterClass, the styles of the system classes, local and global
 * classes of the program's module, and calling a system class's window
 * procedure through CallWindowProc. Left out are the Unicode functions
 * and the *Ex ones, SetLastError and GetLastError, threads, classes of
 * other modules (kernel32 and user32 in Wine's test, which a Windows
 * 3.1 program can't register classes for) and the dialog resource,
 * none of which Windows 3.1 has. Windows 3.1 keeps a class by the
 * module that registered it, so GCW_HMODULE is the module's handle,
 * which isn't the instance's as it is in Win32.
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301, USA
 */

#include "test.h"

#define NUMCLASSWORDS 4

static HINSTANCE main_instance;
static HMODULE main_module;

unsigned get_ds(void);
#pragma aux get_ds = "mov ax,ds" value [ax];

LRESULT CALLBACK __export ClassTest_WndProc(HWND hWnd, UINT msg, WPARAM wParam,
                                            LPARAM lParam)
{
    if (msg == WM_NCCREATE) return 1;
    return DefWindowProc(hWnd, msg, wParam, lParam);
}

/***********************************************************************
 */
static void ClassTest(HINSTANCE hInstance, BOOL global)
{
    WNDCLASS cls, wc;
    static const char className[] = "TestClass";
    static const char winName[] = "WinClassTest";
    HWND hTestWnd;
    long i;
    char str[20];
    ATOM classatom;

    cls.style         = CS_HREDRAW | CS_VREDRAW | (global?CS_GLOBALCLASS:0);
    cls.lpfnWndProc   = ClassTest_WndProc;
    cls.cbClsExtra    = NUMCLASSWORDS*sizeof(DWORD);
    cls.cbWndExtra    = 12;
    cls.hInstance     = hInstance;
    cls.hIcon         = LoadIcon (0, IDI_APPLICATION);
    cls.hCursor       = LoadCursor (0, IDC_ARROW);
    cls.hbrBackground = GetStockObject (WHITE_BRUSH);
    cls.lpszMenuName  = 0;
    cls.lpszClassName = className;

    classatom=RegisterClass(&cls);
    ok(classatom, "failed to register class\n");

    ok(!RegisterClass (&cls),
        "RegisterClass of the same class should fail for the second time\n");

    /* Setup windows */
    hTestWnd = CreateWindow (className, winName,
       WS_OVERLAPPEDWINDOW + WS_HSCROLL + WS_VSCROLL,
       CW_USEDEFAULT, 0, CW_USEDEFAULT, 0, 0,
       0, hInstance, 0);

    ok(hTestWnd!=0, "Failed to create window\n");

    /* test initial values of valid classwords */
    for(i=0; i<NUMCLASSWORDS; i++)
    {
        ok(!GetClassLong(hTestWnd,(int)(i*sizeof (DWORD))),
            "GetClassLong initial value nonzero!\n");
    }

    /* set values of valid class words */
    for(i=0; i<NUMCLASSWORDS; i++)
    {
        ok(!SetClassLong(hTestWnd,(int)(i*sizeof(DWORD)),i+1),
            "GetClassLong(%ld) initial value nonzero!\n",i);
    }

    /* test values of valid classwords that we set */
    for(i=0; i<NUMCLASSWORDS; i++)
    {
        ok( (i+1) == GetClassLong(hTestWnd,(int)(i*sizeof (DWORD))),
            "GetClassLong value doesn't match what was set!\n");
    }

    /* the words of the same bytes: the low word of the first long */
    ok(GetClassWord(hTestWnd, 0) == 1, "GetClassWord(0) is %x\n",
       GetClassWord(hTestWnd, 0));
    ok(SetClassWord(hTestWnd, 2, 0x1234) == 0, "SetClassWord(2) didn't return 0\n");
    ok(GetClassLong(hTestWnd, 0) == 0x12340001L, "GetClassLong(0) is %08lx\n",
       GetClassLong(hTestWnd, 0));

    /* the class's own words */
    ok(GetClassWord(hTestWnd, GCW_CBCLSEXTRA) == NUMCLASSWORDS*sizeof(DWORD),
       "GCW_CBCLSEXTRA is %d\n", GetClassWord(hTestWnd, GCW_CBCLSEXTRA));
    ok(GetClassWord(hTestWnd, GCW_CBWNDEXTRA) == 12,
       "GCW_CBWNDEXTRA is %d\n", GetClassWord(hTestWnd, GCW_CBWNDEXTRA));
    ok(GetClassWord(hTestWnd, GCW_STYLE) == cls.style,
       "GCW_STYLE is %x, not %x\n", GetClassWord(hTestWnd, GCW_STYLE), cls.style);
    ok((HBRUSH)GetClassWord(hTestWnd, GCW_HBRBACKGROUND) == cls.hbrBackground,
       "GCW_HBRBACKGROUND is %x, not %x\n",
       GetClassWord(hTestWnd, GCW_HBRBACKGROUND), cls.hbrBackground);
    ok((HMODULE)GetClassWord(hTestWnd, GCW_HMODULE) == main_module,
       "GCW_HMODULE is %x, not %x\n", GetClassWord(hTestWnd, GCW_HMODULE),
       main_module);
    ok(GetClassLong(hTestWnd, GCL_WNDPROC) == (LONG)cls.lpfnWndProc,
       "GCL_WNDPROC is %08lx, not %08lx\n", GetClassLong(hTestWnd, GCL_WNDPROC),
       (LONG)cls.lpfnWndProc);

    /* check GetClassName */
    i = GetClassName(hTestWnd, str, sizeof(str));
    ok(i == lstrlen(className),
        "GetClassName returned incorrect length\n");
    ok(!lstrcmp(className,str),
        "GetClassName returned incorrect name for this window's class\n");

    /* check GetClassInfo with our hInstance */
    if(GetClassInfo(hInstance, str, &wc))
    {
        ok(wc.cbClsExtra == cls.cbClsExtra,
            "cbClsExtra did not match\n");
        ok(wc.cbWndExtra == cls.cbWndExtra,
            "cbWndExtra did not match\n");
        ok(wc.hbrBackground == cls.hbrBackground,
            "hbrBackground did not match\n");
        ok(wc.hCursor== cls.hCursor,
            "hCursor did not match\n");
        /* The class keeps the module, which GetClassInfo gives. */
        ok(wc.hInstance == (HINSTANCE)main_module || broken(wc.hInstance == cls.hInstance),
            "hInstance did not match %x/%x\n", wc.hInstance, cls.hInstance);
    }
    else
        ok(FALSE,"GetClassInfo (hinstance) failed!\n");

    /* check GetClassInfo with zero hInstance */
    if(global)
    {
        if(GetClassInfo(0, str, &wc))
        {
            ok(wc.cbClsExtra == cls.cbClsExtra,
                "cbClsExtra did not match %x!=%x\n",wc.cbClsExtra,cls.cbClsExtra);
            ok(wc.cbWndExtra == cls.cbWndExtra,
                "cbWndExtra did not match %x!=%x\n",wc.cbWndExtra,cls.cbWndExtra);
            ok(wc.hbrBackground == cls.hbrBackground,
                "hbrBackground did not match %04x!=%04x\n",wc.hbrBackground,cls.hbrBackground);
            ok(wc.hCursor== cls.hCursor,
                "hCursor did not match %04x!=%04x\n",wc.hCursor,cls.hCursor);
        }
        else
            ok(FALSE,"GetClassInfo (0) failed for global class!\n");
    }
    else
    {
        ok(!GetClassInfo(0, str, &wc),
            "GetClassInfo (0) succeeded for local class!\n");
    }

    ok(!UnregisterClass(className, hInstance),
        "Unregister class succeeded with window existing\n");

    ok(DestroyWindow(hTestWnd),
        "DestroyWindow() failed!\n");

    ok(UnregisterClass(className, hInstance),
        "UnregisterClass() failed\n");

    ok(!GetClassInfo(hInstance, className, &wc),
        "GetClassInfo found the class after UnregisterClass\n");

    return;
}

static void check_style( const char *name, int must_exist, UINT style, UINT ignore )
{
    WNDCLASS wc;

    if (GetClassInfo( 0, name, &wc ))
    {
        ok( !(~wc.style & style & ~ignore), "System class %s is missing bits %x (%04x/%04x)\n",
            name, ~wc.style & style, wc.style, style );
        ok( !(wc.style & ~style), "System class %s has extra bits %x (%04x/%04x)\n",
            name, wc.style & ~style, wc.style, style );
    }
    else
        ok( !must_exist, "System class %s does not exist\n", name );
}

/* test styles of system classes */
static void test_styles(void)
{
    /* check style bits */
    check_style( "Button",     1, CS_PARENTDC | CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW, 0 );
    check_style( "ComboBox",   1, CS_PARENTDC | CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW, 0 );
    check_style( "Edit",       1, CS_PARENTDC | CS_DBLCLKS, 0 );
    check_style( "ListBox",    1, CS_PARENTDC | CS_DBLCLKS, CS_PARENTDC /*FIXME*/ );
    check_style( "MDIClient",  1, 0, 0 );
    check_style( "ScrollBar",  1, CS_PARENTDC | CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW, 0 );
    check_style( "Static",     1, CS_PARENTDC | CS_DBLCLKS, 0 );
    check_style( "ComboLBox",  1, CS_SAVEBITS | CS_DBLCLKS, 0 );
    check_style( "DDEMLEvent", 0, 0, 0 );
    check_style( "Message",    0, 0, 0 );
    check_style( "#32768",     1, CS_SAVEBITS | CS_DBLCLKS, 0 );  /* menu */
    check_style( "#32769",     1, CS_DBLCLKS, 0 );  /* desktop */
    check_style( "#32770",     1, CS_SAVEBITS | CS_DBLCLKS, 0 );  /* dialog */
    check_style( "#32772",     1, 0, 0 );  /* icon title */
}

static void check_class_(int line, HINSTANCE inst, const char *name, const char *menu_name)
{
    WNDCLASS wc;
    BOOL ret = GetClassInfo(inst,name,&wc);
    ok_(__FILE__,line)( ret, "Class %s %04x not found\n", name, inst );
    if (ret)
    {
        if (wc.lpszMenuName && menu_name)
            ok_(__FILE__,line)( !lstrcmp( menu_name, wc.lpszMenuName ),
                                "Wrong name %s/%s for class %s %04x\n",
                                wc.lpszMenuName, menu_name, name, inst );
        else
            ok_(__FILE__,line)( !menu_name == !wc.lpszMenuName, "Wrong name %08lx/%08lx for class %s %04x\n",
                                (DWORD)wc.lpszMenuName, (DWORD)menu_name, name, inst );
    }
}
#define check_class(inst,name,menu) check_class_(__LINE__,inst,name,menu)

static void check_instance_( int line, const char *name, HINSTANCE inst, HMODULE gcl_inst )
{
    WNDCLASS wc;
    HWND hwnd;

    ok_(__FILE__,line)( GetClassInfo( inst, name, &wc ), "Couldn't find class %s inst %04x\n", name, inst );
    hwnd = CreateWindow( name, "test_window", 0, 0, 0, 0, 0, 0, 0, inst, 0 );
    ok_(__FILE__,line)( hwnd != NULL, "Couldn't create window for class %s inst %04x\n", name, inst );
    ok_(__FILE__,line)( (HMODULE)GetClassWord( hwnd, GCW_HMODULE ) == gcl_inst,
                        "Wrong GCW module %04x/%04x for class %s\n",
                        GetClassWord( hwnd, GCW_HMODULE ), gcl_inst, name );
    ok_(__FILE__,line)( (HINSTANCE)GetWindowWord( hwnd, GWW_HINSTANCE ) == inst,
                        "Wrong GWW instance %04x/%04x for window %s\n",
                        GetWindowWord( hwnd, GWW_HINSTANCE ), inst, name );
    ok_(__FILE__,line)(!UnregisterClass(name, inst),
                       "UnregisterClass should fail while exists a class window\n");
    DestroyWindow(hwnd);
}
#define check_instance(name,inst,gcl_inst) check_instance_(__LINE__,name,inst,gcl_inst)

/* test various instance parameters */
static void test_instances(void)
{
    WNDCLASS cls, wc;
    HWND hwnd;
    const char *name = "__test__";
    int r;
    char buffer[0x10];

    memset( &cls, 0, sizeof(cls) );
    cls.style         = CS_HREDRAW | CS_VREDRAW;
    cls.lpfnWndProc   = ClassTest_WndProc;
    cls.cbClsExtra    = 0;
    cls.cbWndExtra    = 0;
    cls.lpszClassName = name;

    cls.lpszMenuName  = "main_module";
    cls.hInstance = main_instance;

    ok( RegisterClass( &cls ), "Failed to register local class for main module\n" );
    check_class( main_instance, name, "main_module" );
    check_instance( name, main_instance, main_module );

    hwnd = CreateWindow( name, "test", 0, 0, 0, 0, 0, 0, 0, main_instance, 0 );
    ok( hwnd != 0, "CreateWindow failed\n");

    r = GetClassName( hwnd, buffer, 4 );
    ok( r == 3, "expected 3, got %d\n", r );
    ok( !strcmp( buffer, "__t"), "name wrong: %s\n", buffer );

    /* setting global flag doesn't change status of class */
    SetClassWord( hwnd, GCW_STYLE, CS_GLOBALCLASS );
    ok( !GetClassInfo( 0, name, &wc ), "Class found with null instance\n" );
    SetClassWord( hwnd, GCW_STYLE, CS_HREDRAW | CS_VREDRAW );
    DestroyWindow( hwnd );
    ok( UnregisterClass( name, main_instance ), "Unregister failed for main module\n" );
    ok( !UnregisterClass( name, main_instance ), "Unregister succeeded the second time\n" );

    /* instance doesn't matter for global class */
    cls.style |= CS_GLOBALCLASS;
    cls.lpszMenuName  = "main_module";
    cls.hInstance = main_instance;
    ok( RegisterClass( &cls ), "Failed to register global class for main module\n" );
    ok( !RegisterClass( &cls ), "Succeeded registering global class twice\n" );
    cls.style &= ~CS_GLOBALCLASS;
    cls.lpszMenuName  = "local";
    ok( !RegisterClass( &cls ), "Succeeded registering local class over the global one\n" );

    check_class( main_instance, name, "main_module" );
    check_class( 0, name, "main_module" );
    check_instance( name, main_instance, main_module );
    ok( UnregisterClass( name, main_instance ), "Unregister failed for main module global\n" );
    ok( !UnregisterClass( name, main_instance ), "Unregister succeeded the second time\n" );

    /* check system classes */

    /* we cannot register a global class with the name of a system class */
    cls.style |= CS_GLOBALCLASS;
    cls.lpszMenuName  = "button_main_module";
    cls.lpszClassName = "BUTTON";
    cls.hInstance = main_instance;
    ok( !RegisterClass( &cls ), "Succeeded registering global button class for main module\n" );

    /* local class is OK however */
    cls.style &= ~CS_GLOBALCLASS;
    cls.lpszMenuName  = "button_main_module";
    cls.hInstance = main_instance;
    ok( RegisterClass( &cls ), "Failed to register local button class for main module\n" );
    check_class( main_instance, "BUTTON", "button_main_module" );
    check_instance( "BUTTON", main_instance, main_module );
    ok( UnregisterClass( "BUTTON", main_instance ), "Unregister failed for main module button\n" );

    /* and the system class is there again */
    ok( GetClassInfo( 0, "BUTTON", &wc ), "Button class not found with null instance\n" );
    ok( GetClassInfo( main_instance, "BUTTON", &wc ), "Button class not found with the instance\n" );
    check_class( main_instance, "BUTTON", NULL );
    hwnd = CreateWindow( "BUTTON", "test", 0, 0, 0, 0, 0, 0, 0, main_instance, 0 );
    ok( hwnd != 0, "CreateWindow failed for button\n");
    ok( (HMODULE)GetClassWord( hwnd, GCW_HMODULE ) != main_module,
        "Button class belongs to the program\n" );
    ok( (HMODULE)GetClassWord( hwnd, GCW_HMODULE ) == GetModuleHandle( "USER" ),
        "Button class module %04x, not USER's %04x\n",
        GetClassWord( hwnd, GCW_HMODULE ), GetModuleHandle( "USER" ) );
    DestroyWindow( hwnd );
}

static void test_builtinproc(void)
{
    static const char NORMAL_CLASSES[][10] = {
        "Button",
        "Static",
        "ComboBox",
        "ComboLBox",
        "ListBox",
        "ScrollBar",
        "Edit",
        "#32770",  /* dialog */
    };
    static const int NUM_NORMAL_CLASSES = (sizeof(NORMAL_CLASSES)/sizeof(NORMAL_CLASSES[0]));
    static const char classA[] = "deftest";
    char buf[128];
    WNDPROC oldproc;
    WNDCLASS cls;
    ATOM atom;
    HWND hwnd;
    int i;

    /* a class whose window procedure is DefWindowProc */
    memset(&cls, 0, sizeof(cls));
    cls.hInstance = main_instance;
    cls.hbrBackground = GetStockObject (WHITE_BRUSH);
    cls.lpfnWndProc = (WNDPROC)DefWindowProc;
    cls.lpszClassName = classA;
    atom = RegisterClass(&cls);
    ok(atom != 0, "Couldn't register class\n");

    hwnd = CreateWindow(classA, NULL, 0, 0, 0, 100, 100, NULL, NULL, main_instance, NULL);
    ok(hwnd != NULL, "Couldn't create window\n");
    ok(GetWindowLong(hwnd, GWL_WNDPROC) == (LONG)(WNDPROC)DefWindowProc,
       "Wrong wndproc: %08lx vs %08lx\n", GetWindowLong(hwnd, GWL_WNDPROC),
       (LONG)(WNDPROC)DefWindowProc);
    ok(GetClassLong(hwnd, GCL_WNDPROC) == (LONG)(WNDPROC)DefWindowProc,
       "Wrong class wndproc: %08lx vs %08lx\n", GetClassLong(hwnd, GCL_WNDPROC),
       (LONG)(WNDPROC)DefWindowProc);
    DestroyWindow(hwnd);
    ok(UnregisterClass(classA, main_instance), "UnregisterClass failed\n");

    /* the window procedures of the system classes, called directly */
    for (i = 0; i < NUM_NORMAL_CLASSES; i++)
    {
        WNDPROC procA;
        hwnd = CreateWindow(NORMAL_CLASSES[i], classA, WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 680, 260,
            NULL, NULL, main_instance, 0);
        ok(hwnd != NULL, "Couldn't create window of class %s\n", NORMAL_CLASSES[i]);
        if (!hwnd) continue;
        SetWindowText(hwnd, classA);  /* ComboBox needs this */
        procA = (WNDPROC)GetWindowLong(hwnd, GWL_WNDPROC);
        ok(procA != NULL, "No window procedure for %s\n", NORMAL_CLASSES[i]);
        ok((LONG)procA == GetClassLong(hwnd, GCL_WNDPROC),
           "Window and class procedures differ for %s: %08lx/%08lx\n",
           NORMAL_CLASSES[i], (LONG)procA, GetClassLong(hwnd, GCL_WNDPROC));
        memset(buf, 0, sizeof(buf));
        CallWindowProc((FARPROC)procA, hwnd, WM_GETTEXT, 120, (LPARAM)(LPSTR)buf);
        ok(memcmp(buf, classA, sizeof(classA)) == 0, "WM_GETTEXT invalid return for class %s: %s\n",
           NORMAL_CLASSES[i], (LPSTR)buf);

        oldproc = (WNDPROC)SetWindowLong(hwnd, GWL_WNDPROC, (LONG)ClassTest_WndProc);
        ok(oldproc == procA, "Class %s: SetWindowLong returned %08lx, not %08lx\n",
           NORMAL_CLASSES[i], (LONG)oldproc, (LONG)procA);
        ok(GetWindowLong(hwnd, GWL_WNDPROC) == (LONG)ClassTest_WndProc,
           "Class %s: GWL_WNDPROC is %08lx after SetWindowLong\n",
           NORMAL_CLASSES[i], GetWindowLong(hwnd, GWL_WNDPROC));
        SetWindowLong(hwnd, GWL_WNDPROC, (LONG)oldproc);
        DestroyWindow(hwnd);
    }
}

static const struct
{
    const char name[9];
    int value;
    int badvalue;
} extra_values[] =
{
    {"#32770",30,30}, /* Dialog */
    {"Edit",6,8},
};

static void test_extra_values(void)
{
    int i;
    for(i=0; i< sizeof(extra_values)/sizeof(extra_values[0]); i++)
    {
        WNDCLASS wc;
        BOOL ret = GetClassInfo(NULL,extra_values[i].name,&wc);

        ok( ret, "GetClassInfo (0) failed for global class %s\n", extra_values[i].name);
        if (!ret) continue;
        ok(extra_values[i].value == wc.cbWndExtra,
           "expected %d, got %d\n", extra_values[i].value, wc.cbWndExtra);
    }
}

START_TEST(class)
{
    /* A program's instance handle is its data segment's selector. */
    main_instance = (HINSTANCE)get_ds();
    main_module = GetModuleHandle( "CLASS" );
    ok( main_module != 0, "GetModuleHandle(\"CLASS\") failed\n" );
    ok( GetModuleHandle( (LPCSTR)MAKELP(0, main_instance) ) == main_module,
        "GetModuleHandle of the instance %04x is %04x, not %04x\n", main_instance,
        GetModuleHandle( (LPCSTR)MAKELP(0, main_instance) ), main_module );

    test_extra_values();

    ClassTest(main_instance,FALSE);
    ClassTest(main_instance,TRUE);
    test_styles();
    test_builtinproc();
    test_instances();
}
