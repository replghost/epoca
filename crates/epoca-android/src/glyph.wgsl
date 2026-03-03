struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> viewport: vec2<f32>;

@group(1) @binding(0) var atlas_texture: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let x = (in.position.x / viewport.x) * 2.0 - 1.0;
    let y = 1.0 - (in.position.y / viewport.y) * 2.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.tex_coord = in.tex_coord;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = textureSample(atlas_texture, atlas_sampler, in.tex_coord).r;
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
