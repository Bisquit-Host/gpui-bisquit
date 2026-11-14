// Liquid-glass backdrop, ported from Kyant0/AndroidLiquidGlass (backdrop 2.0.1, Apache-2.0).
// The source texture holds the captured region with its top-left at texel atlas_origin.

struct Backdrop {
    surface0: vec4<f32>,
    surface1: vec4<f32>,
    // alpha, stroke half width, blur sigma, style (1 default, -1 ambient)
    highlight: vec4<f32>,
    // progress, x, y relative to the shape
    glow: vec4<f32>,
    // alpha, blur sigma, offset y, color alpha
    shadow: vec4<f32>,
    inner_shadow: vec4<f32>,
    // RGB and center alpha of an additive radial glow
    radial_glow: vec4<f32>,
    viewport: vec2<f32>,
    draw_origin: vec2<f32>,
    draw_size: vec2<f32>,
    clip_origin: vec2<f32>,
    clip_size: vec2<f32>,
    shape_origin: vec2<f32>,
    shape_size: vec2<f32>,
    // Screen position of the captured texel (0, 0), and the captured size.
    region_offset: vec2<f32>,
    region_size: vec2<f32>,
    source_texture_size: vec2<f32>,
    restore_origin: vec2<f32>,
    restore_texture_size: vec2<f32>,
    // A Compose graphicsLayer scale about the shape's center; shape_* are unscaled.
    layer_scale: vec2<f32>,
    // Scaled content: where the source sits relative to shape_origin.
    content_offset: vec2<f32>,
    // Where the region's texel (0, 0) sits in the shared blur atlas.
    atlas_origin: vec2<f32>,
    radius: f32,
    padding: f32,
    saturation: f32,
    refraction_height: f32,
    refraction_amount: f32,
    depth_effect: f32,
    chromatic_aberration: f32,
    restore: f32,
    blur_sigma: f32,
    radial_glow_edge: f32,
    opacity: f32,
    // The captured region was downsampled by this factor before blurring.
    downscale: f32,
    scale_content: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
}

@group(0) @binding(0) var<uniform> b: Backdrop;
@group(0) @binding(1) var source_texture: texture_2d<f32>;
@group(0) @binding(2) var restore_texture: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;

struct Varying {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_backdrop(@builtin(vertex_index) vertex_id: u32) -> Varying {
    let unit = vec2<f32>(f32(vertex_id & 1u), f32((vertex_id >> 1u) & 1u));
    let pixel = b.draw_origin + unit * b.draw_size;
    var out: Varying;
    out.position = vec4<f32>(pixel / b.viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    return out;
}

// Samples the captured backdrop at a screen position; outside the capture is transparent.
fn sample_region(coord: vec2<f32>) -> vec4<f32> {
    let c = coord - b.region_offset;
    if (any(c < vec2<f32>(0.0)) || any(c > b.region_size)) {
        return vec4<f32>(0.0);
    }
    let downscale = max(b.downscale, 1.0);
    let small = ceil(b.region_size / downscale);
    let t = clamp(c / downscale, vec2<f32>(0.5), max(small - vec2<f32>(0.5), vec2<f32>(0.5)));
    return textureSampleLevel(source_texture, linear_sampler, (t + b.atlas_origin) / b.source_texture_size, 0.0);
}

// Shrinks the source by `downscale`, a power of two: each output texel is the
// box average of its block, one bilinear tap per 2x2 texels.
@fragment
fn fs_down(input: Varying) -> @location(0) vec4<f32> {
    let f = i32(b.downscale);
    let half = max(f / 2, 1);
    let block = floor(input.position.xy - b.atlas_origin) * b.downscale;
    let high = max(b.region_size - vec2<f32>(1.0), vec2<f32>(1.0));
    var sum = vec4<f32>(0.0);
    for (var j = 0; j < half; j += 1) {
        for (var i = 0; i < half; i += 1) {
            let c = clamp(block + vec2<f32>(f32(2 * i + 1), f32(2 * j + 1)), vec2<f32>(1.0), high);
            sum += textureSampleLevel(source_texture, linear_sampler, (c + b.atlas_origin) / b.source_texture_size, 0.0);
        }
    }
    return sum / f32(half * half);
}

fn sample_clamped(coord: vec2<f32>) -> vec4<f32> {
    let c = clamp(coord, vec2<f32>(0.5), b.region_size - vec2<f32>(0.5)) + b.atlas_origin;
    return textureSampleLevel(source_texture, linear_sampler, c / b.source_texture_size, 0.0);
}

// Gaussian with clamp edges, matching Skia's blur image filter. Both axes run in
// one pass: the separable kernel's paired bilinear taps, taken as a product grid.
// Sigma is at most 4 unless the region was too small to shrink; each axis keeps
// its 13 central taps.
@fragment
fn fs_blur(input: Varying) -> @location(0) vec4<f32> {
    let coord = input.position.xy - b.atlas_origin;
    let sigma = b.blur_sigma;
    let taps = i32(ceil(sigma * 3.0));
    let inv = -0.5 / (sigma * sigma);
    var offsets: array<f32, 13>;
    var weights: array<f32, 13>;
    offsets[0] = 0.0;
    weights[0] = 1.0;
    var count = 1;
    for (var i = 1; i <= taps && count < 13; i += 2) {
        let o1 = f32(i);
        let o2 = f32(i + 1);
        let w1 = exp(o1 * o1 * inv);
        let w2 = select(0.0, exp(o2 * o2 * inv), i + 1 <= taps);
        let w = w1 + w2;
        let offset = (o1 * w1 + o2 * w2) / w;
        offsets[count] = offset;
        weights[count] = w;
        offsets[count + 1] = -offset;
        weights[count + 1] = w;
        count += 2;
    }
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = 0; y < count; y += 1) {
        for (var x = 0; x < count; x += 1) {
            let w = weights[x] * weights[y];
            sum += sample_clamped(coord + vec2<f32>(offsets[x], offsets[y])) * w;
            total += w;
        }
    }
    return sum / total;
}

fn sd_rounded_rect(coord: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let corner = abs(coord) - (half_size - vec2<f32>(radius));
    let outside = length(max(corner, vec2<f32>(0.0))) - radius;
    let inside = min(max(corner.x, corner.y), 0.0);
    return outside + inside;
}

fn grad_sd_rounded_rect(coord: vec2<f32>, half_size: vec2<f32>, radius: f32) -> vec2<f32> {
    let corner = abs(coord) - (half_size - vec2<f32>(radius));
    // Not `sign`: on a center line it would zero the gradient.
    let side = select(vec2<f32>(-1.0), vec2<f32>(1.0), coord >= vec2<f32>(0.0));
    // Strict, since a capsule's center row has `corner.y == 0` between its
    // ends, where normalizing `max(corner, 0)` would be NaN.
    if (corner.x > 0.0 || corner.y > 0.0) {
        return side * normalize(max(corner, vec2<f32>(0.0)));
    }
    let grad_x = step(corner.y, corner.x);
    return side * vec2<f32>(grad_x, 1.0 - grad_x);
}

fn circle_map(x: f32) -> f32 {
    return 1.0 - sqrt(1.0 - x * x);
}

fn unpremultiply(c: vec4<f32>) -> vec4<f32> {
    if (c.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, c.a);
}

// Kyant's vibrancy: Android color-controls matrix on unpremultiplied color.
fn saturate_color(c: vec4<f32>) -> vec4<f32> {
    let s = b.saturation;
    if (s == 1.0) {
        return c;
    }
    let u = unpremultiply(c);
    let inv = 1.0 - s;
    let r = 0.213 * inv;
    let g = 0.715 * inv;
    let bl = 0.072 * inv;
    let m = mat3x3<f32>(
        vec3<f32>(r + s, r, r),
        vec3<f32>(g, g + s, g),
        vec3<f32>(bl, bl, bl + s),
    );
    let rgb = clamp(m * u.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb * u.a, u.a);
}

fn over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    return top + bottom * (1.0 - top.a);
}

fn premultiplied(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.14159265) * sigma);
}

fn erf2(v: vec2<f32>) -> vec2<f32> {
    let s = sign(v);
    let a = abs(v);
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    return s - s / (r2 * r2);
}

fn erf1(v: f32) -> f32 {
    return erf2(vec2<f32>(v, 0.0)).x;
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half_size: vec2<f32>) -> f32 {
    let delta = min(half_size.y - corner - abs(y), 0.0);
    let curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

// Coverage of a gaussian-blurred rounded rect, as in GPUI's shadow shader.
fn blurred_rounded_rect(point: vec2<f32>, origin: vec2<f32>, size: vec2<f32>, radius: f32, sigma: f32) -> f32 {
    let half_size = size * 0.5;
    let p = point - (origin + half_size);
    let corner = min(radius, min(half_size.x, half_size.y));
    if (sigma <= 0.0) {
        return clamp(0.5 - sd_rounded_rect(p, half_size, corner), 0.0, 1.0);
    }
    let low = p.y - half_size.y;
    let high = p.y + half_size.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    if (end <= start) {
        return 0.0;
    }
    let step_size = (end - start) / 4.0;
    var y = start + step_size * 0.5;
    var alpha = 0.0;
    for (var i = 0; i < 4; i += 1) {
        alpha += blur_along_x(p.x, p.y - y, sigma, corner, half_size) * gaussian(y, sigma) * step_size;
        y += step_size;
    }
    return alpha;
}

fn layer_scale() -> vec2<f32> {
    return max(b.layer_scale, vec2<f32>(0.0001));
}

// Maps a coordinate in the unscaled layer to the screen.
fn to_screen(local: vec2<f32>) -> vec2<f32> {
    let half_size = b.shape_size * 0.5;
    return b.shape_origin + half_size + (local - half_size) * layer_scale();
}

fn lens_direction(centered: vec2<f32>, half_size: vec2<f32>) -> vec2<f32> {
    let grad_radius = min(b.radius * 1.5, min(half_size.x, half_size.y));
    var dir = grad_sd_rounded_rect(centered, half_size, grad_radius);
    if (b.depth_effect != 0.0 && any(centered != vec2<f32>(0.0))) {
        dir = dir + b.depth_effect * normalize(centered);
    }
    return normalize(dir);
}

// Kyant's lens in the unscaled layer, sampling the backdrop where each layer
// point lands on screen.
fn sample_glass(local: vec2<f32>, centered: vec2<f32>, half_size: vec2<f32>, sd: f32) -> vec4<f32> {
    if (b.scale_content != 0.0) {
        return sample_region(b.shape_origin + b.content_offset + local);
    }
    if (b.refraction_height <= 0.0 || b.refraction_amount <= 0.0 || -sd >= b.refraction_height) {
        return sample_region(to_screen(local));
    }
    let d = circle_map(1.0 - -min(sd, 0.0) / b.refraction_height) * -b.refraction_amount;
    let grad = lens_direction(centered, half_size);
    let refracted = local + d * grad;
    if (b.chromatic_aberration == 0.0) {
        return sample_region(to_screen(refracted));
    }
    let intensity = b.chromatic_aberration * ((centered.x * centered.y) / (half_size.x * half_size.y));
    let dispersed = d * grad * intensity;
    var color = vec4<f32>(0.0);
    let red = sample_region(to_screen(refracted + dispersed));
    color.r += red.r / 3.5;
    color.a += red.a / 7.0;
    let orange = sample_region(to_screen(refracted + dispersed * (2.0 / 3.0)));
    color.r += orange.r / 3.5;
    color.g += orange.g / 7.0;
    color.a += orange.a / 7.0;
    let yellow = sample_region(to_screen(refracted + dispersed * (1.0 / 3.0)));
    color.r += yellow.r / 3.5;
    color.g += yellow.g / 3.5;
    color.a += yellow.a / 7.0;
    let green = sample_region(to_screen(refracted));
    color.g += green.g / 3.5;
    color.a += green.a / 7.0;
    let cyan = sample_region(to_screen(refracted - dispersed * (1.0 / 3.0)));
    color.g += cyan.g / 3.5;
    color.b += cyan.b / 3.0;
    color.a += cyan.a / 7.0;
    let blue = sample_region(to_screen(refracted - dispersed * (2.0 / 3.0)));
    color.b += blue.b / 3.0;
    color.a += blue.a / 7.0;
    let purple = sample_region(to_screen(refracted - dispersed));
    color.r += purple.r / 7.0;
    color.b += purple.b / 3.0;
    color.a += purple.a / 7.0;
    return color;
}

@fragment
fn fs_glass(input: Varying) -> @location(0) vec4<f32> {
    let p = input.position.xy;
    let clip_min = b.clip_origin;
    let clip_max = b.clip_origin + b.clip_size;
    if (any(p < clip_min) || any(p >= clip_max)) {
        discard;
    }

    let half_size = b.shape_size * 0.5;
    let scale = layer_scale();
    let local = (p - b.shape_origin - half_size) / scale + half_size;
    let centered = local - half_size;
    let sd = sd_rounded_rect(centered, half_size, b.radius);
    let coverage = clamp(0.5 - sd * min(scale.x, scale.y), 0.0, 1.0);

    var glass = vec4<f32>(0.0);
    if (coverage > 0.0) {
        var color = saturate_color(sample_glass(local, centered, half_size, sd));
        color = over(premultiplied(b.surface0), color);
        color = over(premultiplied(b.surface1), color);

        // A radial gradient drawn with BlendMode.Plus, as GlassCircleButton's glow.
        if (b.radial_glow.a > 0.0 || b.radial_glow_edge > 0.0) {
            let t = clamp(length(centered) / (min(b.shape_size.x, b.shape_size.y) * 0.5), 0.0, 1.0);
            let alpha = mix(b.radial_glow.a, b.radial_glow_edge, t);
            color = min(color + vec4<f32>(b.radial_glow.rgb * alpha, alpha), vec4<f32>(1.0));
        }

        // InteractiveHighlight: a flat lift plus a radial glow under the finger, both Plus blended.
        let glow = b.glow.x;
        if (glow > 0.0) {
            let radius = min(b.shape_size.x, b.shape_size.y) * 1.5;
            let center = clamp(b.glow.yz, vec2<f32>(0.0), b.shape_size);
            let intensity = smoothstep(radius, radius * 0.5, distance(local, center));
            let lift = 0.08 * glow + 0.15 * glow * intensity;
            color = min(color + vec4<f32>(lift), vec4<f32>(1.0));
        }

        // Highlight.Default: a blurred stroke on the outline, clipped inside, lit at 45 degrees.
        let highlight_alpha = b.highlight.x;
        if (highlight_alpha > 0.0) {
            let half_width = b.highlight.y;
            let sigma = max(b.highlight.z, 0.0001);
            let band = 0.5 * (erf1((half_width - sd) / (sigma * sqrt(2.0)))
                - erf1((-half_width - sd) / (sigma * sqrt(2.0))));
            let grad_radius = min(b.radius * 1.5, min(half_size.x, half_size.y));
            let grad = grad_sd_rounded_rect(centered, half_size, grad_radius);
            let normal = vec2<f32>(0.70710678, 0.70710678);
            let d = dot(grad, normal);
            let lit = abs(d);
            if (b.highlight.w < 0.0) {
                // Highlight.Ambient: white where lit, black opposite, at 38%.
                let alpha = 0.38 * lit * band * highlight_alpha;
                let tone = step(0.0, d);
                color = over(vec4<f32>(vec3<f32>(tone * alpha), alpha), color);
            } else {
                let add = 0.5 * lit * band * highlight_alpha;
                color = min(color + vec4<f32>(add), vec4<f32>(1.0));
            }
        }

        // InnerShadow: the shape minus itself shifted down, blurred, clipped to the shape.
        let inner_alpha = b.inner_shadow.x;
        if (inner_alpha > 0.0) {
            let sigma = b.inner_shadow.y;
            let offset = b.inner_shadow.z;
            let whole = blurred_rounded_rect(local, vec2<f32>(0.0), b.shape_size, b.radius, sigma);
            let shifted_origin = vec2<f32>(0.0, offset);
            let overlap_size = vec2<f32>(b.shape_size.x, max(b.shape_size.y - offset, 0.0));
            let overlap = blurred_rounded_rect(local, shifted_origin, overlap_size, b.radius, sigma);
            let shade = clamp(whole - overlap, 0.0, 1.0) * b.inner_shadow.w * inner_alpha;
            color = over(vec4<f32>(0.0, 0.0, 0.0, shade), color);
        }

        glass = color * coverage;
    }

    // Shadow: blurred shape below the glass, with the shape itself cleared.
    var below = vec4<f32>(0.0);
    let shadow_alpha = b.shadow.x;
    if (shadow_alpha > 0.0 && coverage < 1.0) {
        let shade = blurred_rounded_rect(local, vec2<f32>(0.0, b.shadow.z), b.shape_size, b.radius, b.shadow.y);
        below = vec4<f32>(0.0, 0.0, 0.0, shade * b.shadow.w * shadow_alpha * (1.0 - coverage));
    }

    var result = over(glass, below);
    if (b.restore != 0.0) {
        let restored = textureSampleLevel(restore_texture, linear_sampler,
            (p - b.restore_origin) / b.restore_texture_size, 0.0);
        result = over(result, restored);
    }
    return result * b.opacity;
}

@fragment
fn fs_blit(input: Varying) -> @location(0) vec4<f32> {
    return textureSampleLevel(source_texture, linear_sampler, input.position.xy / b.source_texture_size, 0.0);
}
