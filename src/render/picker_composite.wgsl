// Composite the supersampled picker over the Wayland surface.
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@group(0) @binding(0)
var source: texture_2d<f32>;

struct CompositeInfo {
    origin: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(1)
var<uniform> composite: CompositeInfo;

override render_scale: u32;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let position = array(
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0),
    )[index];
    var output: VertexOutput;
    output.position = vec4(position, 0.0, 1.0);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = floor(input.position.xy - composite.origin);
    let base = vec2<i32>(pixel * f32(render_scale));
    var color = vec4(0.0);
    for (var y = 0u; y < render_scale; y++) {
        for (var x = 0u; x < render_scale; x++) {
            color += textureLoad(source, base + vec2<i32>(i32(x), i32(y)), 0);
        }
    }
    return color / f32(render_scale * render_scale);
}
