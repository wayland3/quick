// 图片编解码（仅 platform 层）
//
// 用途：
// - 把存储的 PNG blob 转成 CF_DIB 写回剪贴板（T-205）
// - 读剪贴板时把 CF_DIB 归一化为 PNG 存储（T-612 读取边界转换）
// 约束：阻塞函数，禁止在 async 上下文直接调用。

use crate::error::AppError;

/// RGBA 像素 → PNG 字节（跨平台；Linux 捕获编码与 Windows DIB 归一化共用）
pub fn rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, AppError> {
    use image::{codecs::png::PngEncoder, ImageBuffer, ImageEncoder, RgbaImage};
    use std::io::Cursor;

    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(AppError::Internal(format!(
            "rgba length mismatch: got {} expect {}",
            rgba.len(),
            expected
        )));
    }

    let buffer: RgbaImage = ImageBuffer::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| AppError::Internal("invalid image dimensions".into()))?;

    let mut png = Vec::new();
    PngEncoder::new(Cursor::new(&mut png))
        .write_image(
            buffer.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| AppError::Internal(format!("PNG encode failed: {}", e)))?;
    Ok(png)
}

/// PNG 字节 → CF_DIB 字节（BITMAPINFOHEADER + bottom-up 32bpp BGRA）
///
/// v1 策略：alpha 统一写 0xFF（GDI 消费方不认 alpha，避免半透明区域发黑）；
/// 行序翻转为 bottom-up，与 `BI_RGB` 正高度一致。
///
/// Linux 不需要 DIB 转换，直接返回 PNG 原始字节。
pub fn png_to_dib(png: &[u8]) -> Result<Vec<u8>, AppError> {
    if png.is_empty() {
        return Err(AppError::ClipboardRead);
    }

    #[cfg(target_os = "windows")]
    {
        png_to_dib_windows(png)
    }

    #[cfg(target_os = "linux")]
    {
        // Linux 不需要 DIB 转换，直接返回 PNG 原始字节
        // services 层写入剪贴板时会走 arboard 的 PNG 路径
        Ok(png.to_vec())
    }
}

/// CF_DIB / CF_DIBV5 原始字节 → PNG 字节（T-612 读取边界归一化）
///
/// 裸 DIB（BITMAPINFOHEADER + 像素）不是文件格式，WebView 与 `image` crate
/// 都不直接认；这里拼上 14 字节 BMP 文件头交给 WIC 解码（覆盖调色板、
/// BI_BITFIELDS、v4/v5 头等剪贴板变体），再编码为 PNG。
/// 失败时调用方回退存原始 DIB（保底不丢图）。
#[cfg(target_os = "windows")]
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, AppError> {
    if dib.is_empty() {
        return Err(AppError::ClipboardRead);
    }

    let (width, height, mut rgba) = wic_decode_bgra(&dib_to_bmp_file(dib)?)?;
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2); // BGRA → RGBA
    }

    // 32bpp BI_RGB 的 DIB 不携带 alpha 语义（GDI 产物的 alpha 字节常为 0）；
    // 全零时按不透明处理，避免转成 PNG 后预览/粘贴变成全透明。
    // 真正带 alpha 的源（BI_BITFIELDS 掩码）不会全零，不受影响。
    if rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 0) {
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 0xFF;
        }
    }

    rgba_to_png(width, height, &rgba)
}

// ============================================================
// Windows 实现
// ============================================================
#[cfg(target_os = "windows")]
fn png_to_dib_windows(png: &[u8]) -> Result<Vec<u8>, AppError> {
    /// BITMAPINFOHEADER 长度
    const BITMAPINFOHEADER_LEN: usize = 40;

    let (width, height, mut pixels) = wic_decode_bgra(png)?;
    let stride = width as usize * 4;

    // WIC 输出 top-down 行序，正高度 `BI_RGB` 要求 bottom-up，逐行反转。
    // 不用负高度（top-down DIB）：老 GDI 消费方对负高度兼容性差。
    flip_rows(&mut pixels, stride);

    // GDI 消费方不认 alpha（半透明区域会渲染发黑），统一写不透明
    for px in pixels.as_chunks_mut::<4>().0 {
        px[3] = 0xFF;
    }

    let mut dib = Vec::with_capacity(BITMAPINFOHEADER_LEN + pixels.len());
    dib.extend_from_slice(&bitmap_info_header(width as usize, height as usize, stride));
    dib.extend_from_slice(&pixels);

    Ok(dib)
}

/// 行序反转（top-down ↔ bottom-up）
#[cfg(target_os = "windows")]
fn flip_rows(pixels: &mut [u8], stride: usize) {
    if stride == 0 {
        return;
    }
    let rows = pixels.len() / stride;
    for row in 0..rows / 2 {
        let mirror = rows - 1 - row;
        let (front, back) = pixels.split_at_mut(mirror * stride);
        front[row * stride..(row + 1) * stride].swap_with_slice(&mut back[..stride]);
    }
}

/// 任意 WIC 支持的图片字节（PNG / BMP 文件）→ 32bpp BGRA 像素
///
/// 返回 `(width, height, bgra)`；行序为 top-down。
#[cfg(target_os = "windows")]
fn wic_decode_bgra(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), AppError> {
    use windows::core::GUID;
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };

    /// 目标像素格式：32bpp BGRA
    #[allow(non_upper_case_globals)]
    const GUID_WICPixelFormat32bppBGRA: GUID =
        GUID::from_u128(0x6fddc324_4e03_4bfe_b185_3d77768dc90f);

    struct ComGuard {
        should_uninit: bool,
    }

    impl ComGuard {
        fn initialize() -> Result<Self, AppError> {
            // SAFETY: 无参数，始终安全
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if hr.is_ok() || hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    should_uninit: hr.is_ok(),
                })
            } else {
                Err(AppError::Internal(format!(
                    "COM init failed: 0x{:08X}",
                    hr.0
                )))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.should_uninit {
                // SAFETY: 与 initialize() 中成功的 CoInitializeEx 一一配对
                unsafe { CoUninitialize() };
            }
        }
    }

    let _com = ComGuard::initialize()?;

    // SAFETY: 所有 COM 对象在本函数内创建与释放，GUID 为静态常量
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| AppError::Internal(format!("WIC factory failed: {}", e)))?;

        let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(bytes))
            .ok_or_else(|| AppError::Internal("SHCreateMemStream failed".into()))?;

        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(|e| AppError::Internal(format!("WIC decoder failed: {}", e)))?;
        let frame = decoder
            .GetFrame(0)
            .map_err(|e| AppError::Internal(format!("WIC frame failed: {}", e)))?;

        let converter = factory
            .CreateFormatConverter()
            .map_err(|e| AppError::Internal(format!("WIC converter create failed: {}", e)))?;

        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| AppError::Internal(format!("WIC convert init failed: {}", e)))?;

        let mut width = 0u32;
        let mut height = 0u32;
        converter
            .GetSize(&mut width, &mut height)
            .map_err(|e| AppError::Internal(format!("WIC size failed: {}", e)))?;

        let stride = width as usize * 4;
        let mut pixels = vec![0u8; stride * height as usize];

        converter
            .CopyPixels(std::ptr::null(), stride as u32, &mut pixels)
            .map_err(|e| AppError::Internal(format!("WIC copy failed: {}", e)))?;

        Ok((width, height, pixels))
    }
}

/// 裸 DIB 前拼 14 字节 BITMAPFILEHEADER，组成 WIC 可解码的 BMP 文件
///
/// `bfOffBits` = 文件头(14) + 信息头(`biSize`) + 掩码 + 调色板：
/// - `BI_BITFIELDS` 且 `biSize == 40` 时，3 个掩码 DWORD 位于信息头与调色板之间；
///   v4/v5 信息头（`biSize >= 52/108`）的掩码已含在 `biSize` 内，不重复加。
/// - `biBitCount <= 8` 时按 `biClrUsed`（0 表示全量 `2^bitCount`）补调色板字节数。
#[cfg(target_os = "windows")]
fn dib_to_bmp_file(dib: &[u8]) -> Result<Vec<u8>, AppError> {
    /// BITMAPFILEHEADER 长度
    const FILE_HEADER_LEN: usize = 14;
    /// BITMAPINFOHEADER（v1）长度
    const INFO_HEADER_V1_LEN: usize = 40;
    const BI_BITFIELDS: u32 = 3;

    if dib.len() < INFO_HEADER_V1_LEN {
        return Err(AppError::ClipboardRead);
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    if !(INFO_HEADER_V1_LEN..=dib.len()).contains(&header_size) {
        return Err(AppError::ClipboardRead);
    }
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]) as usize;
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;

    let mut off_bits = header_size;
    if compression == BI_BITFIELDS && header_size == INFO_HEADER_V1_LEN {
        off_bits += 12; // 3 个颜色掩码 DWORD
    }
    if bit_count <= 8 {
        let colors = if clr_used > 0 {
            clr_used
        } else {
            1 << bit_count
        };
        off_bits += colors * 4;
    }
    if off_bits > dib.len() {
        return Err(AppError::ClipboardRead);
    }

    let mut bmp = Vec::with_capacity(FILE_HEADER_LEN + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((FILE_HEADER_LEN + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 4]); // 两个保留字
    bmp.extend_from_slice(&((FILE_HEADER_LEN + off_bits) as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(bmp)
}

#[cfg(target_os = "windows")]
fn bitmap_info_header(width: usize, height: usize, stride: usize) -> [u8; 40] {
    let mut header = [0u8; 40];
    let bi_size = 40u32;
    let bi_width = width as i32;
    let bi_height = height as i32; // 正数表示 bottom-up
    let bi_planes = 1u16;
    let bi_bit_count = 32u16;
    let bi_compression = 0u32; // BI_RGB
    let bi_size_image = (stride * height) as u32;

    header[0..4].copy_from_slice(&bi_size.to_le_bytes());
    header[4..8].copy_from_slice(&bi_width.to_le_bytes());
    header[8..12].copy_from_slice(&bi_height.to_le_bytes());
    header[12..14].copy_from_slice(&bi_planes.to_le_bytes());
    header[14..16].copy_from_slice(&bi_bit_count.to_le_bytes());
    header[16..20].copy_from_slice(&bi_compression.to_le_bytes());
    header[20..24].copy_from_slice(&bi_size_image.to_le_bytes());

    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(non_snake_case)]
    fn 空PNG返回错误() {
        let result = png_to_dib(&[]);
        assert!(result.is_err());
    }

    #[test]
    #[allow(non_snake_case)]
    fn 非空PNG返回数据() {
        // 1×1 蓝色 PNG（由 System.Drawing 生成的有效 PNG，120 字节）
        const ONE_PX_PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 1, 115, 82, 71, 66, 0, 174, 206, 28, 233, 0,
            0, 0, 4, 103, 65, 77, 65, 0, 0, 177, 143, 11, 252, 97, 5, 0, 0, 0, 9, 112, 72, 89, 115,
            0, 0, 14, 195, 0, 0, 14, 195, 1, 199, 111, 168, 100, 0, 0, 0, 13, 73, 68, 65, 84, 24,
            87, 99, 96, 96, 248, 255, 31, 0, 3, 2, 1, 255, 201, 178, 173, 238, 0, 0, 0, 0, 73, 69,
            78, 68, 174, 66, 96, 130,
        ];
        let result = png_to_dib(ONE_PX_PNG);
        assert!(result.is_ok(), "有效 PNG 应能解码: {:?}", result.err());
        #[cfg(target_os = "linux")]
        assert_eq!(result.unwrap(), ONE_PX_PNG);
    }

    #[test]
    #[allow(non_snake_case)]
    fn rgba编码为PNG后可解码还原() {
        // 2×1：左红右蓝（RGBA）
        let rgba: [u8; 8] = [255, 0, 0, 255, 0, 0, 255, 255];
        let png = rgba_to_png(2, 1, &rgba).expect("编码应成功");

        let decoded = image::load_from_memory(&png).expect("PNG 应可解码");
        let pixels = decoded.to_rgba8();
        assert_eq!(pixels.dimensions(), (2, 1));
        assert_eq!(pixels.as_raw()[0..4], [255, 0, 0, 255]);
        assert_eq!(pixels.as_raw()[4..8], [0, 0, 255, 255]);
    }

    #[test]
    #[allow(non_snake_case)]
    fn rgba长度不符返回错误() {
        assert!(rgba_to_png(2, 1, &[0u8; 4]).is_err());
        assert!(rgba_to_png(0, 0, &[]).is_err()); // 0×0 无效
    }

    #[cfg(target_os = "windows")]
    fn one_px_dib() -> Vec<u8> {
        let mut dib = Vec::new();
        // BITMAPINFOHEADER：40 字节，1×1，32bpp，BI_RGB
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        dib.extend_from_slice(&4u32.to_le_bytes()); // biSizeImage
        dib.extend_from_slice(&[0u8; 16]); // 分辨率/颜色数/重要色数
                                           // 像素：BGRA = 蓝 255、绿 0、红 0、alpha 0（GDI 产物常见）
        dib.extend_from_slice(&[255, 0, 0, 0]);
        dib
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn dib包装出合法BMP文件头() {
        let dib = one_px_dib();
        let bmp = dib_to_bmp_file(&dib).expect("包装应成功");
        assert_eq!(&bmp[0..2], b"BM");
        // bfSize = 14 + dib 长度
        assert_eq!(
            u32::from_le_bytes(bmp[2..6].try_into().unwrap()) as usize,
            14 + dib.len()
        );
        // 32bpp 无调色板：bfOffBits = 14 + 40 = 54
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        assert_eq!(&bmp[14..], &dib[..]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn dib过短或头部非法返回错误() {
        assert!(dib_to_bmp_file(&[]).is_err());
        assert!(dib_to_bmp_file(&[0u8; 10]).is_err());
        let mut bad = one_px_dib();
        bad[0..4].copy_from_slice(&120u32.to_le_bytes()); // biSize=124 但长度不足
        assert!(dib_to_bmp_file(&bad).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn dib转png一像素往返() {
        let png = dib_to_png(&one_px_dib()).expect("转换应成功");
        let decoded = image::load_from_memory(&png).expect("PNG 应可解码");
        let pixels = decoded.to_rgba8();
        assert_eq!(pixels.dimensions(), (1, 1));
        // BGRA(255,0,0,0) → RGBA 红色；alpha 全零按不透明处理
        assert_eq!(pixels.as_raw()[0..4], [0, 0, 255, 255]);
    }

    /// 1×2 双色 DIB：存储序为 bottom-up（首行是视觉底部，蓝色；次行是顶部，红色）
    #[cfg(target_os = "windows")]
    fn two_px_vertical_dib() -> Vec<u8> {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes()); // 宽 1
        dib.extend_from_slice(&2i32.to_le_bytes()); // 高 2（正数 = bottom-up）
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        dib.extend_from_slice(&8u32.to_le_bytes()); // biSizeImage
        dib.extend_from_slice(&[0u8; 16]);
        dib.extend_from_slice(&[255, 0, 0, 255]); // 视觉底部：BGRA 蓝
        dib.extend_from_slice(&[0, 0, 255, 255]); // 视觉顶部：BGRA 红
        dib
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn dib转png保持上下方向() {
        let png = dib_to_png(&two_px_vertical_dib()).expect("转换应成功");
        let decoded = image::load_from_memory(&png).expect("PNG 应可解码");
        let pixels = decoded.to_rgba8();
        assert_eq!(pixels.dimensions(), (1, 2));
        // 首行（视觉顶部）应为红，次行为蓝
        assert_eq!(pixels.as_raw()[0..4], [255, 0, 0, 255]);
        assert_eq!(pixels.as_raw()[4..8], [0, 0, 255, 255]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn png转dib行序翻转为bottomUp() {
        let original = two_px_vertical_dib();
        let png = dib_to_png(&original).expect("转 PNG 应成功");
        let dib = png_to_dib(&png).expect("转 DIB 应成功");

        // 头部：40 字节 + 1×2 正高度 32bpp
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 1);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(dib[16..20].try_into().unwrap()), 0); // BI_RGB

        // 正高度要求 bottom-up 存储：首像素必须是视觉底部（蓝），缺失翻转时会变成红
        assert_eq!(&dib[40..44], &[255, 0, 0, 255], "首行应为视觉底部（蓝）");
        assert_eq!(&dib[44..48], &[0, 0, 255, 255], "次行应为视觉顶部（红）");
    }

    /// 构造指定头字段的 DIB（长度补齐到 off_bits 之后，供文件头包装测试）
    #[cfg(target_os = "windows")]
    fn dib_with_header(
        compression: u32,
        bit_count: u16,
        clr_used: u32,
        total_len: usize,
    ) -> Vec<u8> {
        let mut dib = vec![0u8; total_len];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&1i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&bit_count.to_le_bytes());
        dib[16..20].copy_from_slice(&compression.to_le_bytes());
        dib[32..36].copy_from_slice(&clr_used.to_le_bytes());
        dib
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(non_snake_case)]
    fn 位域与调色板dib的偏移计算正确() {
        // BI_BITFIELDS + v1 40 字节头：bfOffBits = 14 + 40 + 12(掩码) = 66
        let dib = dib_with_header(3, 32, 0, 40 + 12 + 4);
        let bmp = dib_to_bmp_file(&dib).expect("包装应成功");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 66);

        // 8bpp BI_RGB 无 clrUsed：调色板 256×4，bfOffBits = 14 + 40 + 1024 = 1078
        let dib = dib_with_header(0, 8, 0, 40 + 1024 + 4);
        let bmp = dib_to_bmp_file(&dib).expect("包装应成功");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 1078);

        // 8bpp 且 biClrUsed=2：调色板只占 2×4，bfOffBits = 14 + 40 + 8 = 62
        let dib = dib_with_header(0, 8, 2, 40 + 8 + 4);
        let bmp = dib_to_bmp_file(&dib).expect("包装应成功");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 62);
    }
}
