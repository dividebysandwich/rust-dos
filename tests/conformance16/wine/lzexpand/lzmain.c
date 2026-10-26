/*
 * Unit test suite for lz32 functions
 *
 * Copyright 2004 Evan Parry, Daniel Kegel
 *
 * Ported to Win16 from Wine's dlls/lz32/tests/lzexpand_main.c: LZRead
 * and LZCopy of LZEXPAND on a file COMPRESS.EXE packed, as the Windows
 * 3.1 setup programs use them, with KERNEL's OpenFile, _lcreat and
 * _lwrite making the files. The LZOpenFile tests of names, which look
 * at the full paths NT gives and the Unicode functions, are left out.
 * Added here: LZSeek, LZInit on a file that isn't compressed, LZCopy
 * of a DOS handle, and GetExpandedName. The file isn't lzexpand.c: its
 * program would be LZEXPAND.EXE, whose module name is the DLL's.
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

#include <string.h>
#include "test.h"
#include <lzexpand.h>

#pragma library("lzexpand")

/* Compressed file names end with underscore. */
static char filename_[] = "testfile.xx_";
static char filename2[] = "testfile.yyy";

/* This is the hex string representation of the file created by compressing
   a simple text file with the contents "This is a test file."

   The file was created using COMPRESS.EXE from the Windows Server 2003
   Resource Kit from Microsoft.
 */
static const unsigned char compressed_file[] =
  {0x53,0x5A,0x44,0x44,0x88,0xF0,0x27,0x33,0x41,
   0x74,0x75,0x14,0x00,0x00,0xDF,0x54,0x68,0x69,
   0x73,0x20,0xF2,0xF0,0x61,0x20,0xFF,0x74,0x65,
   0x73,0x74,0x20,0x66,0x69,0x6C,0x03,0x65,0x2E};
static const UINT compressed_file_size = sizeof(compressed_file);

static const char uncompressed_data[] = "This is a test file.";
static const UINT uncompressed_data_size = sizeof(uncompressed_data) - 1;

static char buf[64];

static void write_file(const char *name, const void *data, UINT size)
{
  HFILE file;
  UINT ret;

  file = _lcreat(name, 0);
  ok(file != HFILE_ERROR, "Could not create %s\n", name);
  ret = _lwrite(file, data, size);
  ok(ret == size, "Wrote wrong number of bytes with _lwrite: %u\n", ret);
  _lclose(file);
}

static void delete_file(const char *name)
{
  OFSTRUCT ofs;
  HFILE ret;

  ret = OpenFile(name, &ofs, OF_DELETE);
  ok(ret != HFILE_ERROR, "OpenFile(OF_DELETE) failed on %s\n", name);
}

static void test_LZRead(void)
{
  int cfile;
  LONG ret;
  OFSTRUCT test;

  write_file(filename_, compressed_file, compressed_file_size);

  cfile = LZOpenFile(filename_, &test, OF_READ);
  ok(cfile > 0, "LZOpenFile failed\n");

  ret = LZRead(cfile, buf, uncompressed_data_size);
  ok(ret == uncompressed_data_size, "Read wrong number of bytes\n");

  /* Compare what we read with what we think we should read. */
  ok(memcmp(buf, uncompressed_data, uncompressed_data_size) == 0,
     "buffer contents mismatch\n");

  ret = LZRead(cfile, buf, uncompressed_data_size);
  ok(ret == LZERROR_READ, "Expected read-past-EOF to return LZERROR_READ\n");

  /* LZSeek moves in the expanded data. */
  ret = LZSeek(cfile, 5, 0);
  ok(ret == 5, "LZSeek returned %ld\n", ret);
  ret = LZRead(cfile, buf, 2);
  ok(ret == 2 && !memcmp(buf, "is", 2), "Read %ld after LZSeek\n", ret);
  ret = LZSeek(cfile, -2, 1);
  ok(ret == 5, "LZSeek back from the place returned %ld\n", ret);

  LZClose(cfile);
  delete_file(filename_);
}

static void test_LZCopy(void)
{
  int source, dest;
  LONG ret;
  OFSTRUCT stest, dtest;
  HFILE file;

  write_file(filename_, compressed_file, compressed_file_size);

  source = LZOpenFile(filename_, &stest, OF_READ);
  ok(source >= 0, "LZOpenFile failed on compressed file\n");
  dest = LZOpenFile(filename2, &dtest, OF_CREATE);
  ok(dest >= 0, "LZOpenFile failed on creating new file %d\n", dest);

  ret = LZCopy(source, dest);
  ok(ret == uncompressed_data_size, "LZCopy returned %ld\n", ret);

  LZClose(source);
  LZClose(dest);

  file = OpenFile(filename2, &dtest, OF_READ);
  ok(file != HFILE_ERROR, "OpenFile failed on the copy\n");
  ret = _lread(file, buf, uncompressed_data_size * 2);
  ok(ret == uncompressed_data_size, "_lread returned %ld\n", ret);
  /* Compare what we read with what we think we should read. */
  ok(!memcmp(buf, uncompressed_data, uncompressed_data_size),
     "buffer contents mismatch\n");
  _lclose(file);

  delete_file(filename_);
  delete_file(filename2);
}

/* LZCopy of a file's own handle expands it too, as the Microsoft Setup
 * Toolkit's bootstrapper has it, and LZInit gives such a handle back for
 * a file that isn't compressed. */
static void test_LZCopy_handle(void)
{
  HFILE source, dest, file;
  OFSTRUCT ofs;
  LONG ret;

  write_file(filename_, compressed_file, compressed_file_size);
  source = OpenFile(filename_, &ofs, OF_READ);
  ok(source != HFILE_ERROR, "OpenFile failed on compressed file\n");
  dest = _lcreat(filename2, 0);
  ok(dest != HFILE_ERROR, "_lcreat failed\n");
  ret = LZCopy(source, dest);
  ok(ret == uncompressed_data_size, "LZCopy of a DOS handle returned %ld\n", ret);
  _lclose(dest);

  file = OpenFile(filename2, &ofs, OF_READ);
  ok(file != HFILE_ERROR, "OpenFile failed on the copy\n");
  ret = LZInit(file);
  ok(ret == (LONG)file, "LZInit of a plain file returned %ld, not %d\n", ret, file);
  ret = LZRead(file, buf, sizeof(buf));
  ok(ret == uncompressed_data_size, "LZRead of a plain file returned %ld\n", ret);
  ok(!memcmp(buf, uncompressed_data, uncompressed_data_size),
     "buffer contents mismatch\n");
  LZClose(file);

  delete_file(filename_);
  delete_file(filename2);
}

static void test_GetExpandedName(void)
{
  char name[64];
  int ret;

  write_file(filename_, compressed_file, compressed_file_size);
  ret = GetExpandedName(filename_, name);
  ok(ret == 1, "GetExpandedName returned %d\n", ret);
  /* The header keeps 't' as the last character. */
  ok(!lstrcmp(name, "testfile.xxt"), "GetExpandedName gave %s\n", name);
  delete_file(filename_);

  write_file(filename2, uncompressed_data, uncompressed_data_size);
  ret = GetExpandedName(filename2, name);
  ok(ret == 1, "GetExpandedName returned %d\n", ret);
  ok(!lstrcmp(name, filename2), "GetExpandedName gave %s\n", name);
  delete_file(filename2);
}

START_TEST(lzmain)
{
  test_LZRead();
  test_LZCopy();
  test_LZCopy_handle();
  test_GetExpandedName();
}
