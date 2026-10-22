use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use eframe::egui;
use image::DynamicImage;
use rand::rng;
use rand_distr::{Distribution, StandardNormal};
use tiff::encoder::colortype::Gray16;
use tiff::encoder::{Rational, TiffEncoder};
use tiff::tags::ResolutionUnit;

const GENERATE_SHORTCUT: egui::KeyboardShortcut =
	egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::G);

const SAVE_TIFF_SHORTCUT: egui::KeyboardShortcut =
	egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);

const SAVE_DICOM_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
	egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
	egui::Key::S,
);

#[cfg(not(target_arch = "wasm32"))]
const QUIT_SHORTCUT: egui::KeyboardShortcut =
	egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q);

const ZOOM_IN_ICON: egui::ImageSource = egui::include_image!("../assets/icons/zoom_in.svg");

const ZOOM_OUT_ICON: egui::ImageSource = egui::include_image!("../assets/icons/zoom_out.svg");

const FIT_TO_SCREEN_ICON: egui::ImageSource =
	egui::include_image!("../assets/icons/fit_to_screen.svg");

#[derive(Clone, Copy)]
enum MenuCommand {
	Generate,
	SaveTiff,
	SaveDicom,
	#[cfg(not(target_arch = "wasm32"))]
	Quit,
}

fn menu_item(
	ui: &mut egui::Ui,
	label: &str,
	shortcut: egui::KeyboardShortcut,
	enabled: bool,
) -> egui::Response {
	ui.add_enabled(
		enabled,
		egui::Button::new(label).shortcut_text(ui.ctx().format_shortcut(&shortcut)),
	)
}

// White, frameless fill for panels/CentralPanel (the app uses a fixed light look;
// egui's default panel frames would follow the context theme instead).
fn light_panel_frame() -> egui::Frame {
	egui::Frame::new()
		.fill(egui::Color32::WHITE)
		.inner_margin(egui::Margin::symmetric(8, 2))
		.stroke(egui::Stroke::NONE)
}

// 36×36 square icon button used in the Tools panel.
//
// The centered sub-layout is required: the Tools panel layout is left-aligned, so a
// plain `ui.add(...)` would pin the icon to the left inside the square button.
// `Button::new` (not `Button::image`) is used because `Button::image` caps icon
// images at the font height.
fn icon_button(
	ui: &mut egui::Ui,
	icon: egui::ImageSource,
	tooltip: &str,
	enabled: bool,
) -> egui::Response {
	ui.allocate_ui_with_layout(
		egui::Vec2::splat(36.0),
		egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
		|ui| {
			ui.add_enabled(
				enabled,
				egui::Button::new(
					egui::Image::new(icon).fit_to_exact_size(egui::Vec2::splat(20.0)),
				)
				.min_size(egui::Vec2::splat(36.0)),
			)
			.on_hover_text(tooltip)
		},
	)
	.inner
}

// Builds the egui preview texture for an image.
fn make_preview(ctx: &egui::Context, img: &DynamicImage) -> egui::TextureHandle {
	let preview = img.to_rgba8();

	let size = [preview.width() as usize, preview.height() as usize];

	let color_image = egui::ColorImage::from_rgba_unmultiplied(size, preview.as_raw());

	// Zero-order (nearest-neighbor) filtering: zooming duplicates pixels instead
	// of interpolating between them, so the sharp circle edge stays sharp.
	ctx.load_texture(
		"preview",
		color_image,
		egui::TextureOptions {
			magnification: egui::TextureFilter::Nearest,
			minification: egui::TextureFilter::Nearest,
			..Default::default()
		},
	)
}

// Abramowitz & Stegun approximation of the error function (|ε| < 1.5e-7).
fn erf(x: f64) -> f64 {
	let sign = if x < 0.0 { -1.0 } else { 1.0 };

	let x = x.abs();

	let t = 1.0 / (1.0 + 0.3275911 * x);

	let y = 1.0
		- ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t
			+ 0.254829592)
			* t * (-x * x).exp();

	sign * y
}

// Antiderivative of sqrt(r^2 - t^2), valid for t in [-r, r].
fn circle_integral(r: f64, t: f64) -> f64 {
	let t = t.clamp(-r, r);
	(t / 2.0) * (r * r - t * t).max(0.0).sqrt() + (r * r / 2.0) * (t / r).clamp(-1.0, 1.0).asin()
}

// Area of a circle (center at origin, radius r) lying in the lower-left region
// {px <= X, py <= Y}.
fn circle_lower_left_area(r: f64, x: f64, y: f64) -> f64 {
	if r <= 0.0 {
		return 0.0;
	}

	let r2 = r * r;

	let u = x.clamp(-r, r);
	if u <= -r {
		return 0.0;
	}
	if y >= r {
		return 2.0 * (circle_integral(r, u) - circle_integral(r, -r));
	}
	if y <= -r {
		return 0.0;
	}

	// -r < y < r : the horizontal line py = y cuts the circle at px = +/- q.
	let q = (r2 - y * y).max(0.0).sqrt();
	let mut total = 0.0;

	// Outside left: px in [-r, min(-q, u)]  (|px| >= q, h <= |y|).
	let hi = (-q).min(u);
	if hi > -r {
		if y >= 0.0 {
			total += 2.0 * (circle_integral(r, hi) - circle_integral(r, -r));
		}
	}

	// Inside band: px in [max(-r, -q), min(q, u)]  (|px| <= q, h >= |y|).
	let lo2 = (-q).max(-r);
	let hi2 = q.min(u);
	if hi2 > lo2 {
		total += y * (hi2 - lo2) + (circle_integral(r, hi2) - circle_integral(r, lo2));
	}

	// Outside right: px in [q, u] if u > q  (|px| >= q, h <= |y|).
	if u > q {
		if y >= 0.0 {
			total += 2.0 * (circle_integral(r, u) - circle_integral(r, q));
		}
	}

	total
}

// Area of intersection of a circle (center cx, cy, radius r) with the axis-aligned
// rectangle [x0, x1] x [y0, y1], computed by inclusion-exclusion over the
// lower-left cumulative area.
fn circle_rect_area(cx: f64, cy: f64, r: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
	let g = |x: f64, y: f64| circle_lower_left_area(r, x - cx, y - cy);
	g(x1, y1) - g(x0, y1) - g(x1, y0) + g(x0, y0)
}

/// Fills the buffer with a circular test pattern of Gaussian noise:
/// - background at `mean_background`/`stddev_background`,
/// - filled circle of `diameter` pixels at `mean_circle`/`stddev_circle`,
/// - transition between the two governed by a Gaussian with `transition_sigma` (pixels),
/// - cupping: the circle is darker in the middle by `cupping` percent of the
///   mean difference between circle and background (0 % = flat circle); the
///   dip fades to zero toward the circle edge.
///
/// When `transition_sigma` is 0 the edge is sharp: the blend weight equals the
/// exact fraction of each pixel's area covered by the circle, so a boundary
/// cutting a pixel in half gives an intensity that is the area-weighted mean of
/// circle and background (no stair-step). When `transition_sigma` > 0 a Gaussian
/// soft edge is used instead.
pub fn gen_noise(
	image: &mut [u16],
	width: usize,
	height: usize,
	diameter: usize,
	mean_background: f64,
	stddev_background: f64,
	mean_circle: f64,
	stddev_circle: f64,
	cupping: f64,
	transition_sigma: f64,
) -> Result<(), Box<dyn std::error::Error>> {
	if image.len() != width * height {
		return Err("image buffer size does not match width*height".into());
	}

	let mut rng = rng();

	// Break symmetry: shift the circle center off the pixel grid so the edge
	// cuts through pixels at non-integer offsets (useful for testing area
	// coverage at the sharp edge).
	let cx = width as f64 / 2.0 + 0.3;
	let cy = height as f64 / 2.0 + 0.5;

	let radius = diameter as f64 / 2.0;

	// Amount the circle center dips below `mean_circle`.
	let cupping_depth = (mean_circle - mean_background) * cupping / 100.0;

	for y in 0..height {
		let dy = (y as f64 + 0.5) - cy;

		for x in 0..width {
			let dx = (x as f64 + 0.5) - cx;

			let dist = (dx * dx + dy * dy).sqrt();

			// Cupping falloff: 1 (full dip) in the center, 0 at the circle edge.
			let cupping_falloff = 1.0 - (dist / radius).clamp(0.0, 1.0).powi(2);

			// Blend weight `t` = fraction of the pixel's area covered by the circle.
			// - transition_sigma > 0: Gaussian soft edge (existing behaviour).
			// - transition_sigma == 0: sharp edge, but instead of a hard 0/1 step the
			//   weight equals the exact area of circle ∩ pixel, so a boundary cutting a
			//   pixel in half yields t = 0.5 and the intensity is the area-weighted mean
			//   of circle and background (no stair-step).
			let sqrt2_over_2 = std::f64::consts::SQRT_2 / 2.0;
			let t = if transition_sigma > 0.0 {
				0.5 * (1.0 + erf((radius - dist) / (transition_sigma * std::f64::consts::SQRT_2)))
			} else if dist + sqrt2_over_2 <= radius {
				// Entire pixel inside the circle.
				1.0
			} else if dist >= radius + sqrt2_over_2 {
				// Entire pixel outside the circle.
				0.0
			} else {
				let area = circle_rect_area(
					cx,
					cy,
					radius,
					x as f64,
					x as f64 + 1.0,
					y as f64,
					y as f64 + 1.0,
				);
				area.clamp(0.0, 1.0)
			};

			let t = t.clamp(0.0, 1.0);

			let mean =
				(mean_circle - cupping_depth * cupping_falloff) * t + mean_background * (1.0 - t);

			let sigma = stddev_circle * t + stddev_background * (1.0 - t);

			// Generate Gaussian noise:
			let z: f64 = StandardNormal.sample(&mut rng);

			let value = mean + sigma * z;

			image[y * width + x] = value.round().clamp(0.0, 65535.0) as u16;
		}
	}

	Ok(())
}

// Encodes a 16-bit grayscale TIFF with the resolution stored in X/YResolution tags
// and ResolutionUnit. TIFF has no millimeter unit, so pixels/mm/pix mm spacing is
// stored as pixels per centimeter.
fn write_tiff<W: std::io::Write + std::io::Seek>(
	writer: W,
	image: &[u16],
	width: usize,
	height: usize,
	pixel_spacing: f64,
) -> Result<(), Box<dyn std::error::Error>> {
	if image.len() != width * height {
		return Err("image buffer size does not match width*height".into());
	}

	// Pixels per centimeter = 10 / spacing (mm/pix), exact to 0.001 mm.
	let denominator = (pixel_spacing * 1000.0).round().max(1.0) as u32;

	let mut tiff = TiffEncoder::new(writer)?;

	let mut encoder = tiff.new_image::<Gray16>(width as u32, height as u32)?;

	encoder.resolution_unit(ResolutionUnit::Centimeter);

	encoder.x_resolution(Rational { n: 10_000, d: denominator });

	encoder.y_resolution(Rational { n: 10_000, d: denominator });

	encoder.write_data(image)?;

	Ok(())
}

// Saves the image as a 16-bit TIFF with resolution tags (native version).
// Ok(None) means the dialog was canceled, Ok(Some(status)) is the success message.
#[cfg(not(target_arch = "wasm32"))]
fn save_tiff(
	image: &DynamicImage,
	pixel_spacing: f64,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
	let Some(path) = rfd::FileDialog::new()
		.add_filter("TIFF", &["tif", "tiff"])
		.set_file_name("noise_image.tif")
		.save_file()
	else {
		return Ok(None);
	};

	let gray = image.to_luma16();

	let pixels: Vec<u16> = gray.pixels().map(|p| p.0[0]).collect();

	let file = std::fs::File::create(&path)?;

	let mut buffered = std::io::BufWriter::new(file);

	use std::io::Write;

	write_tiff(
		&mut buffered,
		&pixels,
		gray.width() as usize,
		gray.height() as usize,
		pixel_spacing,
	)?;

	buffered.flush()?;

	Ok(Some(format!("Saved {}", path.display())))
}

// Saves the image as a 16-bit TIFF with resolution tags (web version, browser download).
// Ok(None) means the dialog was canceled, Ok(Some(status)) is the success message.
#[cfg(target_arch = "wasm32")]
fn save_tiff(
	image: &DynamicImage,
	pixel_spacing: f64,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
	use std::io::Cursor;

	use eframe::wasm_bindgen::JsCast;
	use web_sys::{Blob, Url};

	let gray = image.to_luma16();

	let pixels: Vec<u16> = gray.pixels().map(|p| p.0[0]).collect();

	let mut cursor = Cursor::new(Vec::<u8>::new());

	write_tiff(&mut cursor, &pixels, gray.width() as usize, gray.height() as usize, pixel_spacing)?;

	let bytes = cursor.into_inner();

	let array = js_sys::Uint8Array::from(bytes.as_slice());

	let seq = js_sys::Array::new();

	seq.push(&array.buffer());

	let blob = Blob::new_with_u8_array_sequence(&seq).unwrap();

	let url = Url::create_object_url_with_blob(&blob).unwrap();

	let document = web_sys::window().unwrap().document().unwrap();

	let a = document.create_element("a").unwrap();

	a.set_attribute("href", &url).unwrap();

	a.set_attribute("download", "noise_image.tif").unwrap();

	let a: web_sys::HtmlElement = a.dyn_into().unwrap();

	a.click();

	Url::revoke_object_url(&url).unwrap();

	Ok(Some("Downloaded noise_image.tif".into()))
}

// Saves the image as DICONDE (DICOM flavor used in NDT/industrial imaging):
// 16-bit grayscale, MONOCHROME2, Explicit VR Little Endian (native version).
// Ok(None) means the dialog was canceled, Ok(Some(status)) is the success message.
#[cfg(not(target_arch = "wasm32"))]
fn save_dicom(
	image: &DynamicImage,
	patient_name: &str,
	pixel_spacing: f64,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
	let Some(path) = rfd::FileDialog::new()
		.add_filter("DICOM/DICONDE", &["dcm"])
		.set_file_name("image.dcm")
		.save_file()
	else {
		return Ok(None);
	};

	let gray = image.to_luma16();

	let mut obj = InMemDicomObject::new_empty();

	obj.put(DataElement::new(Tag(0x0010, 0x0010), VR::PN, PrimitiveValue::from(patient_name)));

	obj.put(DataElement::new(
		Tag(0x0028, 0x0010),
		VR::US,
		PrimitiveValue::from(gray.height() as u16),
	));

	obj.put(DataElement::new(
		Tag(0x0028, 0x0011),
		VR::US,
		PrimitiveValue::from(gray.width() as u16),
	));

	obj.put(DataElement::new(
		Tag(0x0028, 0x0030),
		VR::DS,
		PrimitiveValue::from(format!("{}\\{}", pixel_spacing, pixel_spacing)),
	));

	obj.put(DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0004), VR::CS, PrimitiveValue::from("MONOCHROME2")));

	obj.put(DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(0u16)));

	let pixel_data: Vec<u16> = gray.pixels().map(|p| p.0[0]).collect();

	obj.put(DataElement::new(Tag(0x7FE0, 0x0010), VR::OW, PrimitiveValue::U16(pixel_data.into())));

	let file_obj =
		obj.with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))?;

	file_obj.write_to_file(&path)?;

	Ok(Some(format!("Saved {}", path.display())))
}

// Saves the image as DICONDE (web version, browser download).
// Ok(None) means the dialog was canceled, Ok(Some(status)) is the success message.
#[cfg(target_arch = "wasm32")]
fn save_dicom(
	image: &DynamicImage,
	patient_name: &str,
	pixel_spacing: f64,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
	use std::io::Cursor;

	use eframe::wasm_bindgen::JsCast;
	use web_sys::{Blob, Url};

	let gray = image.to_luma16();

	let mut obj = InMemDicomObject::new_empty();

	obj.put(DataElement::new(Tag(0x0010, 0x0010), VR::PN, PrimitiveValue::from(patient_name)));

	obj.put(DataElement::new(
		Tag(0x0028, 0x0010),
		VR::US,
		PrimitiveValue::from(gray.height() as u16),
	));

	obj.put(DataElement::new(
		Tag(0x0028, 0x0011),
		VR::US,
		PrimitiveValue::from(gray.width() as u16),
	));

	obj.put(DataElement::new(
		Tag(0x0028, 0x0030),
		VR::DS,
		PrimitiveValue::from(format!("{}\\{}", pixel_spacing, pixel_spacing)),
	));

	obj.put(DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0004), VR::CS, PrimitiveValue::from("MONOCHROME2")));

	obj.put(DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)));

	obj.put(DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(0u16)));

	let pixel_data: Vec<u16> = gray.pixels().map(|p| p.0[0]).collect();

	obj.put(DataElement::new(Tag(0x7FE0, 0x0010), VR::OW, PrimitiveValue::U16(pixel_data.into())));

	let file_obj =
		obj.with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))?;

	let mut cursor = Cursor::new(Vec::<u8>::new());

	file_obj.write_all(&mut cursor)?;

	let bytes = cursor.into_inner();

	let array = js_sys::Uint8Array::from(bytes.as_slice());

	let seq = js_sys::Array::new();

	seq.push(&array.buffer());

	let blob = Blob::new_with_u8_array_sequence(&seq).unwrap();

	let url = Url::create_object_url_with_blob(&blob).unwrap();

	let document = web_sys::window().unwrap().document().unwrap();

	let a = document.create_element("a").unwrap();

	a.set_attribute("href", &url).unwrap();

	a.set_attribute("download", "image.dcm").unwrap();

	let a: web_sys::HtmlElement = a.dyn_into().unwrap();

	a.click();

	Url::revoke_object_url(&url).unwrap();

	Ok(Some("Downloaded image.dcm".into()))
}

struct App {
	// Generated image:
	image: Option<DynamicImage>,
	texture: Option<egui::TextureHandle>,

	// Generator parameters (defaults per the r-x-noise spec):
	image_size: usize,      // square image side, pixels (1000)
	background_mean: f64,   // gray level outside the circle (20000)
	background_stddev: f64, // noise std deviation outside the circle (500)
	circle_mean: f64,       // gray level inside the circle (40000)
	circle_stddev: f64,     // noise std deviation inside the circle (1000)
	circle_diameter: usize, // circle diameter, pixels (700)
	cupping: f64,           // dip of the circle center, % of the circle-background difference (0)
	transition_sigma: f64,  // Gaussian transition width, pixels (3.0)

	// Output metadata:
	patient_name: String, // written to DICONDE as Patient Name (0010,0010)
	pixel_spacing: f64,   // resolution, mm/pix

	// Histogram of the generated image: 256 bins over the 0..=65535 gray range,
	// recomputed whenever a new image is stored.
	histogram: Vec<u64>,

	// View: zoom = 1.0 fits the image to the image area; pan offsets it from there.
	zoom: f32,
	pan: egui::Vec2,
	pending_zoom: Option<f32>, // factor requested by the Tools buttons, applied by the image view

	// UI state:
	status: String,
	show_about: bool,
}

impl Default for App {
	fn default() -> Self {
		Self {
			image: None,
			texture: None,
			histogram: Vec::new(),
			image_size: 1000,
			background_mean: 20000.0,
			background_stddev: 500.0,
			circle_mean: 40000.0,
			circle_stddev: 1000.0,
			circle_diameter: 700,
			cupping: 0.0,
			transition_sigma: 3.0,
			patient_name: String::new(),
			pixel_spacing: 0.1,
			zoom: 1.0,
			pan: egui::Vec2::ZERO,
			pending_zoom: None,
			status: String::new(),
			show_about: false,
		}
	}
}

impl App {
	// Stores a newly generated image and builds the preview texture.
	fn set_image(&mut self, image: DynamicImage, ctx: &egui::Context) {
		let texture = make_preview(ctx, &image);

		// Count the gray levels into 256 bins over the 0..=65535 range (each bin
		// covers 256 gray levels; bin = value >> 8):
		let mut histogram = [0u64; 256];

		if let Some(gray) = image.as_luma16() {
			for &value in gray.as_raw() {
				histogram[(value >> 8) as usize] += 1;
			}
		}

		self.histogram = histogram.to_vec();

		self.image = Some(image);

		self.texture = Some(texture);
	}

	// Zooms the image view by `factor` around the fixed screen point `anchor`.
	// `fitted` is where the image is drawn at zoom = 1.
	fn zoom_step(&mut self, fitted: egui::Rect, anchor: egui::Pos2, factor: f32) {
		let display_min = fitted.min + self.pan;

		let display_size = fitted.size() * self.zoom;

		let zoom = (self.zoom * factor).clamp(0.05, 128.0);

		// Where the anchor sits on the image, as a fraction of the displayed area;
		// kept constant, so zooming follows the pointer:
		let u = ((anchor.x - display_min.x) / display_size.x).clamp(0.0, 1.0);

		let v = ((anchor.y - display_min.y) / display_size.y).clamp(0.0, 1.0);

		let new_size = fitted.size() * zoom;

		let new_min = anchor - egui::Vec2::new(u * new_size.x, v * new_size.y);

		self.zoom = zoom;

		self.pan = new_min - fitted.min;
	}

	// Creates the noise test pattern from the current parameters.
	fn generate(&mut self, ctx: &egui::Context) {
		let width = self.image_size;
		let height = self.image_size;
		let diameter = self.circle_diameter;

		if width < 3 || diameter >= width {
			self.status =
				"Circle diameter must be smaller than the image size (>= 3 px)".to_owned();

			return;
		}

		let mut buffer = vec![0u16; width * height];

		if let Err(e) = gen_noise(
			&mut buffer,
			width,
			height,
			diameter,
			self.background_mean,
			self.background_stddev,
			self.circle_mean,
			self.circle_stddev,
			self.cupping,
			self.transition_sigma,
		) {
			self.status = format!("Generation error: {e}");

			return;
		}

		let image = DynamicImage::ImageLuma16(
			image::ImageBuffer::from_raw(width as u32, height as u32, buffer)
				.expect("buffer size matches width*height"),
		);

		self.set_image(image, ctx);

		self.status = format!("Generated {width} × {height} image");
	}

	// Shows the toolbar (menu bar + quick buttons) and handles global keyboard shortcuts.
	// Returns None if no command was triggered this frame.
	fn show_menu(&mut self, ui: &mut egui::Ui) -> Option<MenuCommand> {
		let mut command = None;

		if ui.input_mut(|i| i.consume_shortcut(&GENERATE_SHORTCUT)) {
			command = Some(MenuCommand::Generate);
		}

		if ui.input_mut(|i| i.consume_shortcut(&SAVE_TIFF_SHORTCUT)) {
			command = Some(MenuCommand::SaveTiff);
		}

		if ui.input_mut(|i| i.consume_shortcut(&SAVE_DICOM_SHORTCUT)) {
			command = Some(MenuCommand::SaveDicom);
		}

		#[cfg(not(target_arch = "wasm32"))]
		if ui.input_mut(|i| i.consume_shortcut(&QUIT_SHORTCUT)) {
			command = Some(MenuCommand::Quit);
		}

		let has_image = self.image.is_some();

		let mut open_about = false;

		egui::MenuBar::new().ui(ui, |ui| {
			ui.menu_button("File", |ui| {
				if menu_item(ui, "Generate Image", GENERATE_SHORTCUT, true).clicked() {
					command = Some(MenuCommand::Generate);
				}

				ui.separator();

				#[cfg(target_arch = "wasm32")]
				if menu_item(ui, "Download TIFF...", SAVE_TIFF_SHORTCUT, has_image).clicked() {
					command = Some(MenuCommand::SaveTiff);
				}

				#[cfg(target_arch = "wasm32")]
				if menu_item(ui, "Download DICONDE...", SAVE_DICOM_SHORTCUT, has_image).clicked() {
					command = Some(MenuCommand::SaveDicom);
				}

				#[cfg(not(target_arch = "wasm32"))]
				if menu_item(ui, "Save TIFF Image As...", SAVE_TIFF_SHORTCUT, has_image).clicked() {
					command = Some(MenuCommand::SaveTiff);
				}

				#[cfg(not(target_arch = "wasm32"))]
				if menu_item(ui, "Save DICONDE Image As...", SAVE_DICOM_SHORTCUT, has_image)
					.clicked()
				{
					command = Some(MenuCommand::SaveDicom);
				}

				ui.separator();

				#[cfg(not(target_arch = "wasm32"))]
				if menu_item(ui, "Quit", QUIT_SHORTCUT, true).clicked() {
					command = Some(MenuCommand::Quit);
				}
			});

			ui.menu_button("Help", |ui| {
				if ui.button("About...").clicked() {
					open_about = true;
				}
			});
		});

		if open_about {
			self.show_about = true;
		}

		command
	}

	// Shows the Tools panel (left) — three 36×36 icon buttons, no caption.
	fn show_tools(&mut self, ui: &mut egui::Ui) {
		ui.add_space(4.0);

		let has_image = self.image.is_some();

		if icon_button(ui, ZOOM_IN_ICON, "Zoom In", has_image).clicked() {
			self.pending_zoom = Some(1.25);
		}

		if icon_button(ui, ZOOM_OUT_ICON, "Zoom Out", has_image).clicked() {
			self.pending_zoom = Some(1.0 / 1.25);
		}

		if icon_button(ui, FIT_TO_SCREEN_ICON, "Fit to Screen", has_image).clicked() {
			self.zoom = 1.0;

			self.pan = egui::Vec2::ZERO;
		}
	}

	// Shows the Parameters panel (right) with generator settings and output metadata fields.
	fn show_parameters(&mut self, ui: &mut egui::Ui) -> Option<MenuCommand> {
		ui.label(egui::RichText::new("Parameters").strong().size(14.0));

		ui.add_space(8.0);

		// 2-column grid keeps all numeric editors on one vertical line while the
		// labels stay left-aligned:
		egui::Grid::new("parameters").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
			let tooltip = "Square image side, pixels. Must be larger than the circle diameter";

			ui.label("Image Size").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.image_size).speed(1.0).range(1..=4096),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Mean gray level outside the circle";

			ui.label("Background Mean").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.background_mean)
					.speed(5.0)
					.fixed_decimals(0)
					.range(0.0..=65_535.0),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Noise level (standard deviation) outside the circle, gray levels. 0 = solid fill (no noise)";

			ui.label("Background StdDev").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.background_stddev)
					.speed(1.0)
					.fixed_decimals(0)
					.range(0.0..=32_000.0),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Mean gray level inside the circle";

			ui.label("Circle Mean").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.circle_mean)
					.speed(5.0)
					.fixed_decimals(0)
					.range(0.0..=65_535.0),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Noise level (standard deviation) inside the circle, gray levels. 0 = solid fill (no noise)";

			ui.label("Circle StdDev").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.circle_stddev)
					.speed(1.0)
					.fixed_decimals(0)
					.range(0.0..=32_000.0),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Circle diameter, pixels. must be smaller than the image size";

			ui.label("Circle Diameter").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.circle_diameter).speed(1.0).range(1..=100_000),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Width of the Gaussian transition between circle and background, pixels";

			ui.label("Transition Sigma").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.transition_sigma)
					.speed(0.1)
					.fixed_decimals(1)
					.range(0.0..=100.0),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Cupping. Dips the circle center darker by this percent of the mean \
			               difference between circle and background; the dip vanishes toward \
			               the circle edge (0 % = flat circle)";

			ui.label("Cupping, %").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.cupping)
					.speed(1.0)
					.fixed_decimals(0)
					.range(0.0..=100.0),
			)
			.on_hover_text(tooltip);

			ui.end_row();

			let tooltip = "Resolution. Physical distance in the component between the center of \
			               each pixel, specified internally by a numeric pair: adjacent row \
			               spacing and adjacent column spacing, in mm.";

			ui.label("Resolution").on_hover_text(tooltip);

			ui.add_sized(
				[100.0, 22.0],
				egui::DragValue::new(&mut self.pixel_spacing)
					.speed(0.01)
					.fixed_decimals(6)
					.range(0.0..=f64::INFINITY),
			)
			.on_hover_text(tooltip);
		});

		// Generate button below the numeric controls:
		ui.add_space(6.0);

		let generate_clicked =
			ui.add_sized([ui.available_width(), 30.0], egui::Button::new("Generate")).clicked();

		ui.separator();

		ui.label("Component Name (0010,0010)").on_hover_text("Stored in DICONDE as Patient Name");

		egui::Frame::group(ui.style()).show(ui, |ui| {
			ui.add_sized(
				[ui.available_width(), 24.0],
				egui::TextEdit::singleline(&mut self.patient_name),
			);
		});

		// Histogram — starts right below the parameter content and fills the
		// whole remaining height down to the panel bottom:
		ui.add_space(6.0);

		ui.label(egui::RichText::new("Histogram").strong().size(14.0));

		ui.add_space(4.0);

		if self.histogram.is_empty() {
			ui.weak("Generate an image to show its histogram");
		} else {
			let bars: Vec<egui_plot::Bar> = self
				.histogram
				.iter()
				.enumerate()
				.map(|(bin, count)| {
					egui_plot::Bar::new(bin as f64 + 0.5, *count as f64)
						.width(0.95)
						.fill(egui::Color32::from_rgb(0x5a, 0x8a, 0xc8))
				})
				.collect();

			let chart = egui_plot::BarChart::new("Histogram", bars);

			egui_plot::Plot::new("histogram_plot")
				.height(ui.available_height().max(80.0))
				.x_axis_label("Gray level (bin x 256)")
				.y_axis_label("Count")
				.x_axis_formatter(|mark, _range| format!("{}", (mark.value * 256.0) as u32))
				.show(ui, |plot_ui| {
					plot_ui.bar_chart(chart);
				});
		}

		generate_clicked.then_some(MenuCommand::Generate)
	}

	// Shows the status bar (bottom): image source and the last action result.
	fn show_status(&mut self, ui: &mut egui::Ui) {
		ui.horizontal(|ui| {
			match &self.image {
				Some(image) => ui.label(format!("Image: {} × {}", image.width(), image.height())),
				None => ui.weak("No image generated"),
			};

			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				ui.label(&self.status);
			});
		});
	}
}

impl eframe::App for App {
	fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
		// App title (web only — on desktop the OS window title serves this purpose):
		#[cfg(target_arch = "wasm32")]
		egui::Panel::top(egui::Id::new("title_bar")).frame(light_panel_frame()).show(ui, |ui| {
			ui.label(egui::RichText::new("Noise Test Pattern Generator").strong().size(18.0));
		});

		// Toolbar (menu + quick actions):
		let toolbar = egui::Panel::top(egui::Id::new("toolbar"))
			.frame(light_panel_frame())
			.show(ui, |ui| self.show_menu(ui));

		let menu_command = toolbar.inner;

		// Tools (left) — width fits the 36 px icon buttons plus the 8 px panel margins,
		// so the gaps left and right of each button match:
		egui::Panel::left(egui::Id::new("tools"))
			.frame(light_panel_frame())
			.exact_size(52.0)
			.show(ui, |ui| self.show_tools(ui));

		// Parameters (right) — its Generate button yields a command like the menu does:
		let parameters_command = egui::Panel::right(egui::Id::new("parameters"))
			.frame(light_panel_frame())
			.show(ui, |ui| self.show_parameters(ui))
			.inner;

		let menu_command = menu_command.or(parameters_command);

		// Status bar:
		egui::Panel::bottom(egui::Id::new("status_bar"))
			.frame(light_panel_frame())
			.show(ui, |ui| self.show_status(ui));

		// Main content — the image, with wheel zoom (anchored at the pointer) and pan:
		egui::CentralPanel::default().frame(light_panel_frame()).show(ui, |ui| {
			ui.style_mut().visuals = egui::Visuals::light();

			// Clone handles the borrow: the view state below is mutated while the
			// texture is used for painting.
			let Some(texture) = self.texture.clone() else {
				ui.centered_and_justified(|ui| {
					ui.weak("Press Generate (Ctrl+G) to create the noise test pattern.");
				});

				return;
			};

			let view_rect = ui.available_rect_before_wrap();

			let response = ui.allocate_rect(view_rect, egui::Sense::click_and_drag());

			// Rect the image fills at zoom = 1 (fit to screen, centered):
			let size = texture.size_vec2();

			let fit_scale = (view_rect.width() / size.x).min(view_rect.height() / size.y);

			let fitted = egui::Rect::from_center_size(view_rect.center(), size * fit_scale);

			// Zoom requested by the Tools buttons (around the view center):
			if let Some(factor) = self.pending_zoom.take() {
				self.zoom_step(fitted, view_rect.center(), factor);
			}

			// Zoom on scroll, anchored at the pointer:
			let scroll: f32 = ui.input(|i| i.smooth_scroll_delta.y);

			if response.hovered() && scroll != 0.0 {
				let anchor = ui.input(|i| i.pointer.hover_pos()).unwrap_or(view_rect.center());

				self.zoom_step(fitted, anchor, (1.0 + scroll * 0.005).clamp(0.7, 1.3));
			}

			// Pan by dragging with the mouse:
			if response.dragged() {
				self.pan += response.drag_delta();
			}

			if response.hovered() {
				ui.ctx().set_cursor_icon(if response.dragged() {
					egui::CursorIcon::Grabbing
				} else {
					egui::CursorIcon::Grab
				});
			}

			let display =
				egui::Rect::from_min_size(fitted.min + self.pan, fitted.size() * self.zoom);

			egui::Image::new(&texture).fit_to_exact_size(display.size()).paint_at(ui, display);

			// Green circle overlay — drawn on screen only, never written to the saved file.
			// The image fills `fitted` (centered) at zoom = 1, so the image center maps to
			// `display.center()` and one image pixel covers `fitted.size().x * zoom / size.x`
			// screen pixels.
			let center = display.center();
			let radius = (self.circle_diameter as f32 / 2.0 / size.x) * fitted.size().x * self.zoom;
			if radius.is_finite() && radius > 0.0 {
				ui.painter().add(egui::epaint::CircleShape {
					center,
					radius,
					fill: egui::Color32::TRANSPARENT,
					stroke: egui::Stroke::new(1.0, egui::Color32::GREEN),
				});
			}
		});

		if let Some(command) = menu_command {
			match command {
				MenuCommand::Generate => {
					self.generate(ui.ctx());
				}

				MenuCommand::SaveTiff => {
					if let Some(img) = &self.image {
						match save_tiff(img, self.pixel_spacing) {
							Ok(Some(msg)) => self.status = msg,
							Ok(None) => {}
							Err(e) => self.status = format!("TIFF save error: {e}"),
						}
					}
				}

				MenuCommand::SaveDicom => {
					if let Some(img) = &self.image {
						match save_dicom(img, &self.patient_name, self.pixel_spacing) {
							Ok(Some(msg)) => self.status = msg,
							Ok(None) => {}
							Err(e) => self.status = format!("DICONDE save error: {e}"),
						}
					}
				}

				#[cfg(not(target_arch = "wasm32"))]
				MenuCommand::Quit => {
					ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
				}
			}
		}

		if self.show_about {
			let response = egui::Modal::new(egui::Id::new("about")).show(ui.ctx(), |ui| {
				ui.set_width(280.0);

				ui.vertical_centered(|ui| {
					ui.heading(env!("CARGO_PKG_NAME"));

					ui.weak(concat!("v", env!("CARGO_PKG_VERSION")));

					ui.weak("Noise test pattern generator for ASTM 1695");

					ui.weak("Saves TIFF and DICONDE images");

					ui.hyperlink_to("GitHub", "https://github.com/AndrDm/r-x-noise");
				});

				ui.add_space(8.0);

				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					ui.button("OK").clicked()
				})
				.inner
			});

			if response.inner || response.should_close() {
				self.show_about = false;
			}
		}
	}
}

// desktop version
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
		..Default::default()
	};

	eframe::run_native(
		"Noise Test Pattern Generator",
		options,
		Box::new(|cc| {
			// Use a light theme regardless of the OS theme (set on both styles, since the
			// theme preference decides which one is active at runtime).
			cc.egui_ctx.set_theme(egui::Theme::Light);
			cc.egui_ctx.set_visuals_of(egui::Theme::Dark, egui::Visuals::light());
			cc.egui_ctx.set_visuals_of(egui::Theme::Light, egui::Visuals::light());
			egui_extras::install_image_loaders(&cc.egui_ctx);
			Ok(Box::new(App::default()))
		}),
	)
}

// web version
#[cfg(target_arch = "wasm32")]
fn main() {
	use eframe::wasm_bindgen::JsCast as _;

	let web_options = eframe::WebOptions::default();

	wasm_bindgen_futures::spawn_local(async {
		let document = web_sys::window().expect("No window").document().expect("No document");

		let canvas = document
			.get_element_by_id("the_canvas_id")
			.expect("Failed to find the_canvas_id")
			.dyn_into::<web_sys::HtmlCanvasElement>()
			.expect("the_canvas_id was not a HtmlCanvasElement");

		let start_result = eframe::WebRunner::new()
			.start(
				canvas,
				web_options,
				Box::new(|cc| {
					// Use a light theme regardless of the OS theme (set on both styles, since
					// the theme preference decides which one is active at runtime).
					cc.egui_ctx.set_theme(egui::Theme::Light);
					cc.egui_ctx.set_visuals_of(egui::Theme::Dark, egui::Visuals::light());
					cc.egui_ctx.set_visuals_of(egui::Theme::Light, egui::Visuals::light());
					egui_extras::install_image_loaders(&cc.egui_ctx);
					Ok(Box::new(App::default()))
				}),
			)
			.await;

		let loading_text = web_sys::window()
			.and_then(|w| w.document())
			.and_then(|d| d.get_element_by_id("loading_text"));

		if let Some(loading_text) = loading_text {
			match start_result {
				Ok(_) => loading_text.remove(),
				Err(e) => {
					loading_text.set_inner_html("<p> The app crashed (see console). </p>");
					panic!("Failed to start eframe: {e:?}");
				}
			}
		}
	});
}
