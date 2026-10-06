# Builds test-room.blend and test-room.glb: Rust-DOS's built-in VR test room
# (src/display/stage/scene.rs, Scene::test_room) as a Blender scene, to
# start your own rooms from. See docs/vr.md.
#
#   blender --background --factory-startup --python assets/vr/make_test_room.py
#
# Blender is Z-up and glTF Y-up: the export turns Blender's (x, y, z) into
# glTF's (x, z, -y). The built-in room's glTF positions are converted here.

import math
import os

import bmesh
import bpy
from mathutils import Vector

HERE = os.path.dirname(os.path.abspath(__file__))


def srgb_to_linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def linear_to_srgb(c):
    c = max(0.0, min(1.0, c))
    return c * 12.92 if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def hex_linear(value):
    """The sRGB colour #rrggbb in linear light, as Blender's colour fields take it."""
    return [srgb_to_linear(((value >> shift) & 0xFF) / 255) for shift in (16, 8, 0)]


def from_gltf(x, y, z):
    """A glTF (Y-up) position in Blender's Z-up coordinates."""
    return Vector((x, -z, y))


def material(name, color, emission=None, strength=1.0, texture=None):
    """A Principled BSDF of the base colour (linear RGB), and the emission."""
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    mat.use_backface_culling = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*color, 1.0)
    bsdf.inputs["Roughness"].default_value = 0.8
    if emission:
        bsdf.inputs["Emission Color"].default_value = (*emission, 1.0)
        bsdf.inputs["Emission Strength"].default_value = strength
    if texture:
        node = mat.node_tree.nodes.new("ShaderNodeTexImage")
        node.image = texture
        node.location = (-400, 200)
        mat.node_tree.links.new(node.outputs["Color"], bsdf.inputs["Base Color"])
    return mat


def link(obj, collection):
    collection.objects.link(obj)
    return obj


def cuboid(name, center, size, mat, collection):
    """A box of `size` (Blender's x, y, z) around `center`, its origin there."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    for v in bm.verts:
        v.co = Vector((v.co.x * size[0], v.co.y * size[1], v.co.z * size[2]))
    bm.to_mesh(mesh)
    bm.free()
    mesh.materials.append(mat)
    obj = bpy.data.objects.new(name, mesh)
    obj.location = center
    return link(obj, collection)


def quad(name, center, corners, uvs, mat, collection):
    """A quad of four corners around `center` (anticlockwise seen from its front)."""
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([Vector(c) for c in corners], [], [(0, 1, 2, 3)])
    layer = mesh.uv_layers.new(name="UVMap")
    for loop, uv in zip(mesh.loops, uvs):
        layer.data[loop.index].uv = uv
    mesh.materials.append(mat)
    obj = bpy.data.objects.new(name, mesh)
    obj.location = center
    return link(obj, collection)


def empty(name, location, collection, kind="PLAIN_AXES", size=0.1):
    obj = bpy.data.objects.new(name, None)
    obj.empty_display_type = kind
    obj.empty_display_size = size
    obj.location = location
    return link(obj, collection)


def grid_texture():
    """The floor's metre grid: dark, with thin blue-grey lines at the tile's
    edges, as the built-in room's floor shader draws them up close."""
    size = 256
    image = bpy.data.images.new("floor-grid", size, size, alpha=False)
    image.colorspace_settings.name = "sRGB"
    floor = hex_linear(0x1A1C20)
    # The shader mixes the floor 80% towards this at a line.
    line = [f * 0.2 + l * 0.8 for f, l in zip(floor, (0.05, 0.07, 0.12))]
    floor_srgb = [linear_to_srgb(c) for c in floor] + [1.0]
    line_srgb = [linear_to_srgb(c) for c in line] + [1.0]
    pixels = []
    for y in range(size):
        for x in range(size):
            on_line = min(x, size - 1 - x) < 2 or min(y, size - 1 - y) < 2
            pixels.extend(line_srgb if on_line else floor_srgb)
    image.pixels.foreach_set(pixels)
    image.file_format = "PNG"
    image.pack()
    return image


README = """Rust-DOS test room
==================

The built-in VR test room of Rust-DOS as a Blender scene, to start rooms
of your own from. docs/vr.md in the Rust-DOS sources explains everything.

What Rust-DOS looks for, by object name:
  screen                  the mesh the DOS picture is drawn on (its UVs place it)
  spawn                   where your eyes start, looking along its +Y
  speaker_left/right      where the left and right sound comes from
  led_power, led_turbo,   the PC's lights: they glow in their material's
  led_hdd, led_floppy     emission colour while the emulated PC's are lit

Export: File > Export > glTF 2.0
  Format: glTF Binary (.glb)
  Include: Custom Properties on; Punctual Lights off (see below)
  Transform: +Y Up
  Mesh: Apply Modifiers
  Lighting Mode: Unitless

The Sun lamp is here so that Blender's viewport looks like the room. With
Punctual Lights off, Rust-DOS lights the room with its own sunset, exactly
as the built-in room; with them on, by this lamp (and less of the sky's
light).

The floor's grid is a texture here; the built-in room draws it in a
shader, so far away it looks a little different.

Load it: rust-dos --vr-desktop --vr-scene test-room.glb
"""


def build():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.name = "Test room"
    scene.unit_settings.system = "METRIC"
    # Scene custom properties Rust-DOS reads (these are the defaults).
    scene["rustdos_sky"] = True
    scene["rustdos_exposure"] = 1.0

    room = bpy.data.collections.new("Room")
    pc = bpy.data.collections.new("PC")
    markers = bpy.data.collections.new("Rust-DOS markers")
    preview = bpy.data.collections.new("Viewport only")
    for collection in (room, pc, markers, preview):
        scene.collection.children.link(collection)

    # The floor: 500 m square, so its edges are lost in the haze; one UV
    # tile a metre.
    edge = 250.0
    floor = material("Floor", [1.0, 1.0, 1.0], texture=grid_texture())
    quad(
        "Floor",
        Vector((0, 0, 0)),
        [(-edge, -edge, 0), (edge, -edge, 0), (edge, edge, 0), (-edge, edge, 0)],
        [(-edge, -edge), (edge, -edge), (edge, edge), (-edge, edge)],
        floor,
        room,
    )

    # The screen: 4:3, 1.6 m wide, 2.5 m ahead, its middle 1.4 m up, facing
    # the viewer (-Y). UV (0, 1) is the picture's top left.
    w, h = 1.6, 1.2
    center = from_gltf(0.0, 1.4, -2.5)
    glass = material("Screen (ignored: shows the picture)", hex_linear(0x05070A), emission=(0.02, 0.03, 0.05))
    quad(
        "screen",
        center,
        [(-w / 2, 0, h / 2), (-w / 2, 0, -h / 2), (w / 2, 0, -h / 2), (w / 2, 0, h / 2)],
        [(0, 1), (0, 0), (1, 0), (1, 1)],
        glass,
        room,
    )
    # Its frame: a thin dark slab behind it.
    cuboid("Bezel", from_gltf(0.0, 1.4, -2.535), (w + 0.08, 0.06, h + 0.08), material("Bezel", hex_linear(0x101114)), room)

    # The tower, floating beside it, its front to the viewer.
    tower = (1.2, 1.0, -2.4)
    tw, th, td = 0.2, 0.44, 0.42
    case = material("Case", hex_linear(0x2C2D31))
    slot = material("Drive slots", hex_linear(0x0B0B0D))
    cuboid("Tower", from_gltf(*tower), (tw, td, th), case, pc)
    front = tower[2] + td / 2

    def on_front(x, y):
        return from_gltf(tower[0] + x, tower[1] + y, front)

    for i, (y, height) in enumerate([(0.17, 0.045), (0.115, 0.045)]):
        cuboid(f"Drive bay {i + 1}", on_front(0.0, y), (0.16, 0.004, height), slot, pc)
    cuboid("Floppy drive", on_front(0.0, 0.06), (0.11, 0.004, 0.028), slot, pc)

    def led(name, color):
        # Dark, a tint of its colour; lit, its colour at strength 1.6.
        return material(name, [c * 0.15 for c in color], emission=color, strength=1.6)

    cuboid("led_floppy", on_front(0.045, 0.052), (0.008, 0.008, 0.005), led("LED floppy", (0.1, 1.0, 0.15)), pc)
    for i, (name, color) in enumerate(
        [("power", (0.1, 1.0, 0.15)), ("turbo", (1.0, 0.65, 0.05)), ("hdd", (1.0, 0.12, 0.05))]
    ):
        x = -0.05 + i * 0.025
        cuboid(f"led_{name}", on_front(x, 0.0), (0.008, 0.008, 0.008), led(f"LED {name}", color), pc)

    # Where the eyes start (1.2 m up, looking at the screen along +Y), and
    # where the sound comes from: the screen's sides, as without them.
    # (Its arrows show the way: Rust-DOS looks along the empty's +Y.)
    empty("spawn", Vector((0, 0, 1.2)), markers, kind="ARROWS", size=0.3)
    empty("speaker_left", center + Vector((-w / 2, 0, 0)), markers, kind="SPHERE", size=0.05)
    empty("speaker_right", center + Vector((w / 2, 0, 0)), markers, kind="SPHERE", size=0.05)

    # For the viewport: the sunset sun (the direction the built-in room's
    # comes from), a camera at the spawn with its 60 degree view, and a
    # dark blue sky.
    sun_data = bpy.data.lights.new("Sun", "SUN")
    sun_data.color = (1.0, 0.5, 0.22)
    sun_data.energy = 1.6
    sun = link(bpy.data.objects.new("Sun", sun_data), preview)
    toward = from_gltf(-0.45, 0.1, -1.0).normalized()
    sun.rotation_euler = toward.to_track_quat("Z", "Y").to_euler()
    camera_data = bpy.data.cameras.new("View")
    camera_data.sensor_fit = "VERTICAL"
    camera_data.angle = math.radians(60)
    camera = link(bpy.data.objects.new("View", camera_data), preview)
    camera.location = (0, 0, 1.2)
    camera.rotation_euler = (math.radians(90), 0, 0)
    scene.camera = camera
    world = bpy.data.worlds.new("Sunset")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.012, 0.028, 0.1, 1.0)
    scene.world = world

    text = bpy.data.texts.new("README")
    text.write(README)

    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(HERE, "test-room.blend"), compress=True)
    bpy.ops.export_scene.gltf(
        filepath=os.path.join(HERE, "test-room.glb"),
        export_format="GLB",
        export_extras=True,
        export_yup=True,
        export_apply=True,
        export_lights=False,
        export_cameras=False,
        export_import_convert_lighting_mode="COMPAT",
    )


build()
