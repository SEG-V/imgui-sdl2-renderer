//! SDL2-based imgui renderer implementation.
//! Note: Requires SDL2 version 2.0.20+

use std::mem::size_of;
use std::ptr::null_mut;

use imgui::internal::RawWrapper;

use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Texture, TextureCreator, WindowCanvas};
use sdl2::sys::{SDL_Color, SDL_RenderGeometryRaw, SDL_ScaleMode, SDL_SetTextureScaleMode};
use sdl2::video::WindowContext;

const RGBA32_BYTES: u32 = 4; // 4 bytes per pixel

struct BackupSDLRendererState {
    clip_rect: Option<Rect>,
    viewport: Rect,
}

pub struct Renderer<'a> {
    texture_map: imgui::Textures<Texture<'a>>,
}

impl<'a> Renderer<'a> {
    pub fn new(
        canvas: &'a mut WindowCanvas,
        imgui_context: &mut imgui::Context,
        texture_creator: &'a TextureCreator<WindowContext>,
    ) -> Result<Self, String> {
        imgui_context
            .set_renderer_name(format!("imgui-sdl2-renderer {}", env!("CARGO_PKG_VERSION")));
        imgui_context
            .io_mut()
            .backend_flags
            .insert(imgui::BackendFlags::RENDERER_HAS_VTX_OFFSET);

        let mut fonts = imgui_context.fonts();

        let imgui::FontAtlasTexture {
            data: pixels,
            height,
            width,
        } = fonts.build_rgba32_texture();

        let mut font_texture = texture_creator
            .create_texture_static(PixelFormatEnum::RGBA32, width, height)
            .map_err(|error| error.to_string())?;

        font_texture
            .update(None, pixels, (width * RGBA32_BYTES) as _)
            .map_err(|error| error.to_string())?;
        canvas.set_blend_mode(BlendMode::Blend);
        font_texture.set_blend_mode(BlendMode::Blend);

        unsafe {
            SDL_SetTextureScaleMode(font_texture.raw(), SDL_ScaleMode::SDL_ScaleModeLinear);
        }

        let mut texture_map = imgui::Textures::new();

        fonts.tex_id = texture_map.insert(font_texture);

        Ok(Self { texture_map })
    }

    pub fn render(
        &self,
        canvas: &'a mut WindowCanvas,
        draw_data: imgui::DrawData,
    ) -> Result<(), String> {
        let (rsx, rsy) = canvas.scale();
        let render_scale = [
            if rsx == 1.0 {
                draw_data.framebuffer_scale[0]
            } else {
                1.0
            },
            if rsy == 1.0 {
                draw_data.framebuffer_scale[1]
            } else {
                1.0
            },
        ];

        let fb_height = draw_data.display_size[1] * render_scale[1];
        let fb_width = draw_data.display_size[0] * render_scale[0];
        if !(fb_width > 0.0 && fb_height > 0.0) {
            return Ok(());
        }

        let backup = BackupSDLRendererState {
            clip_rect: canvas.clip_rect(),
            viewport: canvas.viewport(),
        };

        let clip_off = draw_data.display_pos;
        let clip_scale = render_scale;

        for draw_list in draw_data.draw_lists() {
            let idx_buffer: &[imgui::DrawIdx] = draw_list.idx_buffer();
            let vtx_buffer = draw_list.vtx_buffer();

            for command in draw_list.commands() {
                match command {
                    imgui::DrawCmd::Elements { count, cmd_params } => {
                        let mut clip_min = [
                            (cmd_params.clip_rect[0] - clip_off[0]) * clip_scale[0],
                            (cmd_params.clip_rect[1] - clip_off[1]) * clip_scale[1],
                        ];
                        let mut clip_max = [
                            (cmd_params.clip_rect[2] - clip_off[0]) * clip_scale[0],
                            (cmd_params.clip_rect[3] - clip_off[1]) * clip_scale[1],
                        ];

                        if clip_min[0] < 0.0 {
                            clip_min[0] = 0.0;
                        }
                        if clip_min[1] < 0.0 {
                            clip_min[1] = 0.0;
                        }
                        if clip_max[0] > fb_width {
                            clip_max[0] = fb_width;
                        }
                        if clip_max[1] > fb_height {
                            clip_max[1] = fb_height;
                        }
                        if clip_max[0] <= clip_min[0] || clip_max[1] <= clip_min[1] {
                            continue;
                        }

                        unsafe {
                            let rect = Rect::new(
                                clip_min[0] as _,
                                clip_min[1] as _,
                                (clip_max[0] - clip_min[0]) as u32,
                                (clip_max[1] - clip_min[1]) as u32,
                            );
                            canvas.set_clip_rect(rect);

                            let vtx_buffer_ptr = vtx_buffer.as_ptr();
                            let idx_buffer_ptr = idx_buffer.as_ptr();

                            let position_field_offset = (vtx_buffer_ptr.add(cmd_params.vtx_offset)
                                as usize)
                                + memoffset::offset_of!(imgui::DrawVert, pos);
                            let uv_field_offset = (vtx_buffer_ptr.add(cmd_params.vtx_offset)
                                as usize)
                                + memoffset::offset_of!(imgui::DrawVert, uv);
                            let color_field_offset = (vtx_buffer_ptr.add(cmd_params.vtx_offset)
                                as usize)
                                + memoffset::offset_of!(imgui::DrawVert, col);

                            let font_texture = self.texture_map.get(cmd_params.texture_id);

                            SDL_RenderGeometryRaw(
                                canvas.raw(),
                                match font_texture {
                                    Some(texture) => texture.raw(),
                                    None => null_mut(),
                                },
                                position_field_offset as *const f32,
                                size_of::<imgui::DrawVert>() as _,
                                color_field_offset as *const SDL_Color,
                                size_of::<imgui::DrawVert>() as _,
                                uv_field_offset as *const f32,
                                size_of::<imgui::DrawVert>() as _,
                                (vtx_buffer.len() - cmd_params.vtx_offset) as _,
                                idx_buffer_ptr.add(cmd_params.idx_offset).cast(),
                                count as _,
                                size_of::<imgui::DrawIdx>() as _,
                            );
                        }
                    }
                    imgui::DrawCmd::RawCallback { callback, raw_cmd } => unsafe {
                        callback(draw_list.raw(), raw_cmd)
                    },
                    imgui::DrawCmd::ResetRenderState => Self::setup_render_state(canvas),
                }
            }
        }

        canvas.set_clip_rect(backup.clip_rect);
        canvas.set_viewport(backup.viewport);
        Ok(())
    }

    pub fn setup_render_state(canvas: &mut WindowCanvas) {
        canvas.set_clip_rect(None);
        canvas.set_viewport(None);
    }
}
