//! Drawing the overlay inside a Vulkan game (`vkQueuePresentKHR` hook on
//! Windows and Proton).
//!
//! ## How hooking works (W^X-safe)
//! Once `vulkan-1.dll` is loaded, we IAT-patch `vkGetInstanceProcAddr`,
//! `vkGetDeviceProcAddr`, `vkCreateWin32SurfaceKHR`, `vkDestroySurfaceKHR`,
//! `vkCreateDevice`, `vkDestroyDevice`, `vkCreateSwapchainKHR`,
//! `vkDestroySwapchainKHR`, and `vkQueuePresentKHR` across all loaded game
//! modules (never our own DLL or `vulkan-1.dll`). Any pointer resolved through
//! `vkGetInstanceProcAddr` / `vkGetDeviceProcAddr` is intercepted and wrapped.
//!
//! ## Constraint for `GetProcAddress`-only Vulkan loaders
//! Games that load `vulkan-1.dll` dynamically via `LoadLibrary` +
//! `GetProcAddress("vkGetInstanceProcAddr")` (e.g., `volk`, SDL2/GLFW dynamic
//! Vulkan loaders) bypass IAT entries. Intercepting those games requires a
//! separate Khronos implicit/explicit Vulkan layer JSON (`VK_LAYER_AURORA_overlay`)
//! registered in `HKCU\Software\Khronos\Vulkan\ImplicitLayers` (Windows) or via
//! `VK_LAYER_PATH` + `VK_INSTANCE_LAYERS` (Proton/Linux). When our in-process
//! hook does not observe a `vkQueuePresentKHR` frame, Aurora automatically
//! keeps its window overlay fallback active with zero regressions.

/// Pre-compiled, `spirv-val`-verified SPIR-V shaders for the Vulkan overlay.
/// Kept platform-neutral so unit tests validate the embedded binaries on Linux
/// as well as Windows.
pub mod spv {
    /// Vertex shader: maps egui point coordinates to Vulkan NDC (top-down Y).
    pub const VS_SPV: &[u32] = &[
        0x07230203, 0x00010000, 0x0008000b, 0x00000035, 0x00000000, 0x00020011, 0x00000001, 0x0006000b,
        0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
        0x000b000f, 0x00000000, 0x00000004, 0x6e69616d, 0x00000000, 0x0000000d, 0x00000013, 0x0000002e,
        0x0000002f, 0x00000031, 0x00000033, 0x00030003, 0x00000002, 0x000001c2, 0x00040005, 0x00000004,
        0x6e69616d, 0x00000000, 0x00060005, 0x0000000b, 0x505f6c67, 0x65567265, 0x78657472, 0x00000000,
        0x00060006, 0x0000000b, 0x00000000, 0x505f6c67, 0x7469736f, 0x006e6f69, 0x00070006, 0x0000000b,
        0x00000001, 0x505f6c67, 0x746e696f, 0x657a6953, 0x00000000, 0x00070006, 0x0000000b, 0x00000002,
        0x435f6c67, 0x4470696c, 0x61747369, 0x0065636e, 0x00070006, 0x0000000b, 0x00000003, 0x435f6c67,
        0x446c6c75, 0x61747369, 0x0065636e, 0x00030005, 0x0000000d, 0x00000000, 0x00040005, 0x00000013,
        0x705f6e69, 0x0000736f, 0x00040005, 0x00000019, 0x68737550, 0x00000000, 0x00050006, 0x00000019,
        0x00000000, 0x65726373, 0x00006e65, 0x00030005, 0x0000001b, 0x00006370, 0x00040005, 0x0000002e,
        0x5f74756f, 0x00007675, 0x00040005, 0x0000002f, 0x755f6e69, 0x00000076, 0x00040005, 0x00000031,
        0x5f74756f, 0x006c6f63, 0x00040005, 0x00000033, 0x635f6e69, 0x00006c6f, 0x00030047, 0x0000000b,
        0x00000002, 0x00050048, 0x0000000b, 0x00000000, 0x0000000b, 0x00000000, 0x00050048, 0x0000000b,
        0x00000001, 0x0000000b, 0x00000001, 0x00050048, 0x0000000b, 0x00000002, 0x0000000b, 0x00000003,
        0x00050048, 0x0000000b, 0x00000003, 0x0000000b, 0x00000004, 0x00040047, 0x00000013, 0x0000001e,
        0x00000000, 0x00030047, 0x00000019, 0x00000002, 0x00050048, 0x00000019, 0x00000000, 0x00000023,
        0x00000000, 0x00040047, 0x0000002e, 0x0000001e, 0x00000000, 0x00040047, 0x0000002f, 0x0000001e,
        0x00000001, 0x00040047, 0x00000031, 0x0000001e, 0x00000001, 0x00040047, 0x00000033, 0x0000001e,
        0x00000002, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016, 0x00000006,
        0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000004, 0x00040015, 0x00000008, 0x00000020,
        0x00000000, 0x0004002b, 0x00000008, 0x00000009, 0x00000001, 0x0004001c, 0x0000000a, 0x00000006,
        0x00000009, 0x0006001e, 0x0000000b, 0x00000007, 0x00000006, 0x0000000a, 0x0000000a, 0x00040020,
        0x0000000c, 0x00000003, 0x0000000b, 0x0004003b, 0x0000000c, 0x0000000d, 0x00000003, 0x00040015,
        0x0000000e, 0x00000020, 0x00000001, 0x0004002b, 0x0000000e, 0x0000000f, 0x00000000, 0x0004002b,
        0x00000006, 0x00000010, 0x40000000, 0x00040017, 0x00000011, 0x00000006, 0x00000002, 0x00040020,
        0x00000012, 0x00000001, 0x00000011, 0x0004003b, 0x00000012, 0x00000013, 0x00000001, 0x0004002b,
        0x00000008, 0x00000014, 0x00000000, 0x00040020, 0x00000015, 0x00000001, 0x00000006, 0x0003001e,
        0x00000019, 0x00000011, 0x00040020, 0x0000001a, 0x00000009, 0x00000019, 0x0004003b, 0x0000001a,
        0x0000001b, 0x00000009, 0x00040020, 0x0000001c, 0x00000009, 0x00000006, 0x0004002b, 0x00000006,
        0x00000020, 0x3f800000, 0x0004002b, 0x00000006, 0x00000029, 0x00000000, 0x00040020, 0x0000002b,
        0x00000003, 0x00000007, 0x00040020, 0x0000002d, 0x00000003, 0x00000011, 0x0004003b, 0x0000002d,
        0x0000002e, 0x00000003, 0x0004003b, 0x00000012, 0x0000002f, 0x00000001, 0x0004003b, 0x0000002b,
        0x00000031, 0x00000003, 0x00040020, 0x00000032, 0x00000001, 0x00000007, 0x0004003b, 0x00000032,
        0x00000033, 0x00000001, 0x00050036, 0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8,
        0x00000005, 0x00050041, 0x00000015, 0x00000016, 0x00000013, 0x00000014, 0x0004003d, 0x00000006,
        0x00000017, 0x00000016, 0x00050085, 0x00000006, 0x00000018, 0x00000010, 0x00000017, 0x00060041,
        0x0000001c, 0x0000001d, 0x0000001b, 0x0000000f, 0x00000014, 0x0004003d, 0x00000006, 0x0000001e,
        0x0000001d, 0x00050088, 0x00000006, 0x0000001f, 0x00000018, 0x0000001e, 0x00050083, 0x00000006,
        0x00000021, 0x0000001f, 0x00000020, 0x00050041, 0x00000015, 0x00000022, 0x00000013, 0x00000009,
        0x0004003d, 0x00000006, 0x00000023, 0x00000022, 0x00050085, 0x00000006, 0x00000024, 0x00000010,
        0x00000023, 0x00060041, 0x0000001c, 0x00000025, 0x0000001b, 0x0000000f, 0x00000009, 0x0004003d,
        0x00000006, 0x00000026, 0x00000025, 0x00050088, 0x00000006, 0x00000027, 0x00000024, 0x00000026,
        0x00050083, 0x00000006, 0x00000028, 0x00000027, 0x00000020, 0x00070050, 0x00000007, 0x0000002a,
        0x00000021, 0x00000028, 0x00000029, 0x00000020, 0x00050041, 0x0000002b, 0x0000002c, 0x0000000d,
        0x0000000f, 0x0003003e, 0x0000002c, 0x0000002a, 0x0004003d, 0x00000011, 0x00000030, 0x0000002f,
        0x0003003e, 0x0000002e, 0x00000030, 0x0004003d, 0x00000007, 0x00000034, 0x00000033, 0x0003003e,
        0x00000031, 0x00000034, 0x000100fd, 0x00010038,
    ];

    /// Fragment shader: samples egui texture with optional sRGB-to-linear conversion.
    pub const FS_SPV: &[u32] = &[
        0x07230203, 0x00010000, 0x0008000b, 0x00000078, 0x00000000, 0x00020011, 0x00000001, 0x0006000b,
        0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e, 0x00000000, 0x0003000e, 0x00000000, 0x00000001,
        0x0008000f, 0x00000004, 0x00000004, 0x6e69616d, 0x00000000, 0x0000002f, 0x00000038, 0x00000058,
        0x00030010, 0x00000004, 0x00000007, 0x00030003, 0x00000002, 0x000001c2, 0x00040005, 0x00000004,
        0x6e69616d, 0x00000000, 0x00040005, 0x0000002f, 0x635f6e69, 0x00006c6f, 0x00040005, 0x00000034,
        0x65745f75, 0x00000078, 0x00040005, 0x00000038, 0x755f6e69, 0x00000076, 0x00040005, 0x0000003c,
        0x68737550, 0x00000000, 0x00060006, 0x0000003c, 0x00000000, 0x656e696c, 0x6f5f7261, 0x00007475,
        0x00030005, 0x0000003e, 0x00006370, 0x00050005, 0x00000058, 0x5f74756f, 0x6f6c6f63, 0x00000072,
        0x00040047, 0x0000002f, 0x0000001e, 0x00000001, 0x00040047, 0x00000034, 0x00000021, 0x00000000,
        0x00040047, 0x00000034, 0x00000022, 0x00000000, 0x00040047, 0x00000038, 0x0000001e, 0x00000000,
        0x00030047, 0x0000003c, 0x00000002, 0x00050048, 0x0000003c, 0x00000000, 0x00000023, 0x00000008,
        0x00040047, 0x00000058, 0x0000001e, 0x00000000, 0x00020013, 0x00000002, 0x00030021, 0x00000003,
        0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000003,
        0x00020014, 0x0000000d, 0x00040017, 0x0000000e, 0x0000000d, 0x00000003, 0x0004002b, 0x00000006,
        0x00000012, 0x3d25aee6, 0x0006002c, 0x00000007, 0x00000013, 0x00000012, 0x00000012, 0x00000012,
        0x0004002b, 0x00000006, 0x00000017, 0x3d6147ae, 0x0006002c, 0x00000007, 0x00000018, 0x00000017,
        0x00000017, 0x00000017, 0x0004002b, 0x00000006, 0x0000001d, 0x4019999a, 0x0006002c, 0x00000007,
        0x0000001e, 0x0000001d, 0x0000001d, 0x0000001d, 0x00040017, 0x0000002b, 0x00000006, 0x00000004,
        0x00040020, 0x0000002e, 0x00000001, 0x0000002b, 0x0004003b, 0x0000002e, 0x0000002f, 0x00000001,
        0x00090019, 0x00000031, 0x00000006, 0x00000001, 0x00000000, 0x00000000, 0x00000000, 0x00000001,
        0x00000000, 0x0003001b, 0x00000032, 0x00000031, 0x00040020, 0x00000033, 0x00000000, 0x00000032,
        0x0004003b, 0x00000033, 0x00000034, 0x00000000, 0x00040017, 0x00000036, 0x00000006, 0x00000002,
        0x00040020, 0x00000037, 0x00000001, 0x00000036, 0x0004003b, 0x00000037, 0x00000038, 0x00000001,
        0x0003001e, 0x0000003c, 0x00000006, 0x00040020, 0x0000003d, 0x00000009, 0x0000003c, 0x0004003b,
        0x0000003d, 0x0000003e, 0x00000009, 0x00040020, 0x00000041, 0x00000009, 0x00000006, 0x0004002b,
        0x00000006, 0x00000044, 0x3f000000, 0x00040020, 0x00000057, 0x00000003, 0x0000002b, 0x0004003b,
        0x00000057, 0x00000058, 0x00000003, 0x0004002b, 0x00000006, 0x0000006b, 0x3f72a76f, 0x0006002c,
        0x00000007, 0x0000006c, 0x0000006b, 0x0000006b, 0x0000006b, 0x0004002b, 0x00000006, 0x0000006d,
        0x3d9e8391, 0x0006002c, 0x00000007, 0x0000006e, 0x0000006d, 0x0000006d, 0x0000006d, 0x00040015,
        0x00000075, 0x00000020, 0x00000000, 0x0004002b, 0x00000075, 0x00000076, 0x00000000, 0x00050036,
        0x00000002, 0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005, 0x0004003d, 0x0000002b,
        0x00000030, 0x0000002f, 0x0004003d, 0x00000032, 0x00000035, 0x00000034, 0x0004003d, 0x00000036,
        0x00000039, 0x00000038, 0x00050057, 0x0000002b, 0x0000003a, 0x00000035, 0x00000039, 0x00050085,
        0x0000002b, 0x0000003b, 0x00000030, 0x0000003a, 0x00050041, 0x00000041, 0x00000042, 0x0000003e,
        0x00000076, 0x0004003d, 0x00000006, 0x00000043, 0x00000042, 0x000500ba, 0x0000000d, 0x00000045,
        0x00000043, 0x00000044, 0x000300f7, 0x00000047, 0x00000000, 0x000400fa, 0x00000045, 0x00000046,
        0x00000047, 0x000200f8, 0x00000046, 0x0008004f, 0x00000007, 0x0000004a, 0x0000003b, 0x0000003b,
        0x00000000, 0x00000001, 0x00000002, 0x000500bc, 0x0000000e, 0x00000060, 0x0000004a, 0x00000013,
        0x00050081, 0x00000007, 0x00000062, 0x0000004a, 0x00000018, 0x00050085, 0x00000007, 0x00000063,
        0x00000062, 0x0000006c, 0x0007000c, 0x00000007, 0x00000064, 0x00000001, 0x0000001a, 0x00000063,
        0x0000001e, 0x00050085, 0x00000007, 0x00000066, 0x0000004a, 0x0000006e, 0x000600a9, 0x00000007,
        0x0000006a, 0x00000060, 0x00000066, 0x00000064, 0x00050051, 0x00000006, 0x00000050, 0x0000006a,
        0x00000000, 0x00060052, 0x0000002b, 0x00000070, 0x00000050, 0x0000003b, 0x00000000, 0x00050051,
        0x00000006, 0x00000053, 0x0000006a, 0x00000001, 0x00060052, 0x0000002b, 0x00000072, 0x00000053,
        0x00000070, 0x00000001, 0x00050051, 0x00000006, 0x00000056, 0x0000006a, 0x00000002, 0x00060052,
        0x0000002b, 0x00000074, 0x00000056, 0x00000072, 0x00000002, 0x000200f9, 0x00000047, 0x000200f8,
        0x00000047, 0x000700f5, 0x0000002b, 0x00000077, 0x0000003b, 0x00000005, 0x00000074, 0x00000046,
        0x0003003e, 0x00000058, 0x00000077, 0x000100fd, 0x00010038,
    ];
}

#[cfg(windows)]
mod win_impl {
    use super::spv::{FS_SPV, VS_SPV};
    use crate::overlay::util::{self, Backend};
    use crate::overlay::wire::VERTEX_SIZE;
    use crate::overlay::{Frame, TexOp, shared};
    use std::collections::HashMap;
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::Win32::Foundation::HWND;
    use windows::core::{s, w};

    // ---- Minimal Vulkan 1.0 + WSI ABI types -----------------------------

    type VkResult = i32;
    type VkBool32 = u32;
    type VkFlags = u32;
    type VkDeviceSize = u64;
    type VkInstance = *mut c_void;
    type VkPhysicalDevice = *mut c_void;
    type VkDevice = *mut c_void;
    type VkQueue = *mut c_void;
    type VkCommandBuffer = *mut c_void;
    type VkSurfaceKHR = u64;
    type VkSwapchainKHR = u64;
    type VkImage = u64;
    type VkImageView = u64;
    type VkBuffer = u64;
    type VkDeviceMemory = u64;
    type VkShaderModule = u64;
    type VkRenderPass = u64;
    type VkFramebuffer = u64;
    type VkPipelineLayout = u64;
    type VkPipeline = u64;
    type VkDescriptorSetLayout = u64;
    type VkDescriptorPool = u64;
    type VkDescriptorSet = u64;
    type VkSampler = u64;
    type VkCommandPool = u64;
    type VkSemaphore = u64;
    type VkFence = u64;

    const VK_SUCCESS: VkResult = 0;
    const VK_TRUE: VkBool32 = 1;
    const VK_FALSE: VkBool32 = 0;

    const VK_STRUCTURE_TYPE_SUBMIT_INFO: i32 = 4;
    const VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO: i32 = 5;
    const VK_STRUCTURE_TYPE_FENCE_CREATE_INFO: i32 = 8;
    const VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO: i32 = 9;
    const VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO: i32 = 12;
    const VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO: i32 = 14;
    const VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO: i32 = 15;
    const VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO: i32 = 16;
    const VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO: i32 = 18;
    const VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: i32 = 19;
    const VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: i32 = 20;
    const VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO: i32 = 22;
    const VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO: i32 = 23;
    const VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: i32 = 24;
    const VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: i32 = 26;
    const VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO: i32 = 27;
    const VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO: i32 = 28;
    const VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO: i32 = 30;
    const VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO: i32 = 31;
    const VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO: i32 = 32;
    const VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO: i32 = 33;
    const VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO: i32 = 34;
    const VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET: i32 = 35;
    const VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO: i32 = 37;
    const VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO: i32 = 38;
    const VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO: i32 = 39;
    const VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO: i32 = 40;
    const VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO: i32 = 42;
    const VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO: i32 = 43;
    const VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER: i32 = 45;

    const VK_FORMAT_R8G8B8A8_UNORM: i32 = 37;
    const VK_FORMAT_R32G32_SFLOAT: i32 = 103;
    const VK_IMAGE_LAYOUT_UNDEFINED: i32 = 0;
    const VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL: i32 = 2;
    const VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL: i32 = 5;
    const VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL: i32 = 7;
    const VK_IMAGE_LAYOUT_PRESENT_SRC_KHR: i32 = 1000001002;
    const VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT: VkFlags = 0x1;
    const VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT: VkFlags = 0x80;
    const VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT: VkFlags = 0x400;
    const VK_PIPELINE_STAGE_TRANSFER_BIT: VkFlags = 0x1000;
    const VK_ACCESS_SHADER_READ_BIT: VkFlags = 0x20;
    const VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT: VkFlags = 0x100;
    const VK_ACCESS_TRANSFER_WRITE_BIT: VkFlags = 0x1000;
    const VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT: VkFlags = 0x1;
    const VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT: VkFlags = 0x2;
    const VK_MEMORY_PROPERTY_HOST_COHERENT_BIT: VkFlags = 0x4;
    const VK_BUFFER_USAGE_TRANSFER_SRC_BIT: VkFlags = 0x1;
    const VK_BUFFER_USAGE_INDEX_BUFFER_BIT: VkFlags = 0x40;
    const VK_BUFFER_USAGE_VERTEX_BUFFER_BIT: VkFlags = 0x80;
    const VK_IMAGE_USAGE_TRANSFER_DST_BIT: VkFlags = 0x2;
    const VK_IMAGE_USAGE_SAMPLED_BIT: VkFlags = 0x4;
    const VK_SHADER_STAGE_VERTEX_BIT: VkFlags = 0x1;
    const VK_SHADER_STAGE_FRAGMENT_BIT: VkFlags = 0x10;
    const VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER: i32 = 1;
    const VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT: VkFlags = 0x1;
    const VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT: VkFlags = 0x2;
    const VK_FENCE_CREATE_SIGNALED_BIT: VkFlags = 0x1;
    const VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: VkFlags = 0x1;
    const VK_IMAGE_ASPECT_COLOR_BIT: VkFlags = 0x1;
    const VK_INDEX_TYPE_UINT32: i32 = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkExtent2D { width: u32, height: u32 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkExtent3D { width: u32, height: u32, depth: u32 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkOffset2D { x: i32, y: i32 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkOffset3D { x: i32, y: i32, z: i32 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkRect2D { offset: VkOffset2D, extent: VkExtent2D }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkViewport { x: f32, y: f32, width: f32, height: f32, min_depth: f32, max_depth: f32 }

    #[repr(C)]
    struct VkWin32SurfaceCreateInfoKHR {
        s_type: i32,
        p_next: *const c_void,
        flags: VkFlags,
        hinstance: *mut c_void,
        hwnd: HWND,
    }

    #[repr(C)]
    struct VkDeviceQueueCreateInfo {
        s_type: i32,
        p_next: *const c_void,
        flags: VkFlags,
        queue_family_index: u32,
        queue_count: u32,
        p_queue_priorities: *const f32,
    }

    #[repr(C)]
    struct VkDeviceCreateInfo {
        s_type: i32,
        p_next: *const c_void,
        flags: VkFlags,
        queue_create_info_count: u32,
        p_queue_create_infos: *const VkDeviceQueueCreateInfo,
        enabled_layer_count: u32,
        pp_enabled_layer_names: *const *const c_char,
        enabled_extension_count: u32,
        pp_enabled_extension_names: *const *const c_char,
        p_enabled_features: *const c_void,
    }

    #[repr(C)]
    struct VkSwapchainCreateInfoKHR {
        s_type: i32,
        p_next: *const c_void,
        flags: VkFlags,
        surface: VkSurfaceKHR,
        min_image_count: u32,
        image_format: i32,
        image_color_space: i32,
        image_extent: VkExtent2D,
        image_array_layers: u32,
        image_usage: VkFlags,
        image_sharing_mode: i32,
        queue_family_index_count: u32,
        p_queue_family_indices: *const u32,
        pre_transform: i32,
        composite_alpha: i32,
        present_mode: i32,
        clipped: VkBool32,
        old_swapchain: VkSwapchainKHR,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkPresentInfoKHR {
        s_type: i32,
        p_next: *const c_void,
        wait_semaphore_count: u32,
        p_wait_semaphores: *const VkSemaphore,
        swapchain_count: u32,
        p_swapchains: *const VkSwapchainKHR,
        p_image_indices: *const u32,
        p_results: *mut VkResult,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct VkMemoryType { property_flags: VkFlags, heap_index: u32 }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct VkMemoryHeap { size: VkDeviceSize, flags: VkFlags }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkPhysicalDeviceMemoryProperties {
        memory_type_count: u32,
        memory_types: [VkMemoryType; 32],
        memory_heap_count: u32,
        memory_heaps: [VkMemoryHeap; 16],
    }

    #[repr(C)]
    #[derive(Default)]
    struct VkMemoryRequirements { size: VkDeviceSize, alignment: VkDeviceSize, memory_type_bits: u32 }
    #[repr(C)]
    struct VkMemoryAllocateInfo { s_type: i32, p_next: *const c_void, allocation_size: VkDeviceSize, memory_type_index: u32 }
    #[repr(C)]
    struct VkBufferCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, size: VkDeviceSize,
        usage: VkFlags, sharing_mode: i32, queue_family_index_count: u32, p_queue_family_indices: *const u32,
    }
    #[repr(C)]
    struct VkImageCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, image_type: i32, format: i32,
        extent: VkExtent3D, mip_levels: u32, array_layers: u32, samples: u32, tiling: i32,
        usage: VkFlags, sharing_mode: i32, queue_family_index_count: u32, p_queue_family_indices: *const u32,
        initial_layout: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VkImageSubresourceRange { aspect_mask: VkFlags, base_mip_level: u32, level_count: u32, base_array_layer: u32, layer_count: u32 }
    #[repr(C)]
    struct VkComponentMapping { r: i32, g: i32, b: i32, a: i32 }
    #[repr(C)]
    struct VkImageViewCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, image: VkImage, view_type: i32,
        format: i32, components: VkComponentMapping, subresource_range: VkImageSubresourceRange,
    }
    #[repr(C)]
    struct VkShaderModuleCreateInfo { s_type: i32, p_next: *const c_void, flags: VkFlags, code_size: usize, p_code: *const u32 }
    #[repr(C)]
    struct VkAttachmentDescription {
        flags: VkFlags, format: i32, samples: u32, load_op: i32, store_op: i32,
        stencil_load_op: i32, stencil_store_op: i32, initial_layout: i32, final_layout: i32,
    }
    #[repr(C)]
    struct VkAttachmentReference { attachment: u32, layout: i32 }
    #[repr(C)]
    struct VkSubpassDescription {
        flags: VkFlags, pipeline_bind_point: i32, input_attachment_count: u32, p_input_attachments: *const VkAttachmentReference,
        color_attachment_count: u32, p_color_attachments: *const VkAttachmentReference,
        p_resolve_attachments: *const VkAttachmentReference, p_depth_stencil_attachment: *const VkAttachmentReference,
        preserve_attachment_count: u32, p_preserve_attachments: *const u32,
    }
    #[repr(C)]
    struct VkSubpassDependency {
        src_subpass: u32, dst_subpass: u32, src_stage_mask: VkFlags, dst_stage_mask: VkFlags,
        src_access_mask: VkFlags, dst_access_mask: VkFlags, dependency_flags: VkFlags,
    }
    #[repr(C)]
    struct VkRenderPassCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, attachment_count: u32,
        p_attachments: *const VkAttachmentDescription, subpass_count: u32,
        p_subpasses: *const VkSubpassDescription, dependency_count: u32, p_dependencies: *const VkSubpassDependency,
    }
    #[repr(C)]
    struct VkFramebufferCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, render_pass: VkRenderPass,
        attachment_count: u32, p_attachments: *const VkImageView, width: u32, height: u32, layers: u32,
    }
    #[repr(C)]
    struct VkDescriptorSetLayoutBinding {
        binding: u32, descriptor_type: i32, descriptor_count: u32, stage_flags: VkFlags, p_immutable_samplers: *const VkSampler,
    }
    #[repr(C)]
    struct VkDescriptorSetLayoutCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, binding_count: u32, p_bindings: *const VkDescriptorSetLayoutBinding,
    }
    #[repr(C)]
    struct VkDescriptorPoolSize { ty: i32, descriptor_count: u32 }
    #[repr(C)]
    struct VkDescriptorPoolCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, max_sets: u32, pool_size_count: u32, p_pool_sizes: *const VkDescriptorPoolSize,
    }
    #[repr(C)]
    struct VkDescriptorSetAllocateInfo {
        s_type: i32, p_next: *const c_void, descriptor_pool: VkDescriptorPool, descriptor_set_count: u32, p_set_layouts: *const VkDescriptorSetLayout,
    }
    #[repr(C)]
    struct VkDescriptorImageInfo { sampler: VkSampler, image_view: VkImageView, image_layout: i32 }
    #[repr(C)]
    struct VkWriteDescriptorSet {
        s_type: i32, p_next: *const c_void, dst_set: VkDescriptorSet, dst_binding: u32, dst_array_element: u32,
        descriptor_count: u32, descriptor_type: i32, p_image_info: *const VkDescriptorImageInfo,
        p_buffer_info: *const c_void, p_texel_buffer_view: *const c_void,
    }
    #[repr(C)]
    struct VkSamplerCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, mag_filter: i32, min_filter: i32, mipmap_mode: i32,
        address_mode_u: i32, address_mode_v: i32, address_mode_w: i32, mip_lod_bias: f32,
        anisotropy_enable: VkBool32, max_anisotropy: f32, compare_enable: VkBool32, compare_op: i32,
        min_lod: f32, max_lod: f32, border_color: i32, unnormalized_coordinates: VkBool32,
    }
    #[repr(C)]
    struct VkPushConstantRange { stage_flags: VkFlags, offset: u32, size: u32 }
    #[repr(C)]
    struct VkPipelineLayoutCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, set_layout_count: u32, p_set_layouts: *const VkDescriptorSetLayout,
        push_constant_range_count: u32, p_push_constant_ranges: *const VkPushConstantRange,
    }
    #[repr(C)]
    struct VkPipelineShaderStageCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, stage: VkFlags, module: VkShaderModule,
        p_name: *const c_char, p_specialization_info: *const c_void,
    }
    #[repr(C)]
    struct VkVertexInputBindingDescription { binding: u32, stride: u32, input_rate: i32 }
    #[repr(C)]
    struct VkVertexInputAttributeDescription { location: u32, binding: u32, format: i32, offset: u32 }
    #[repr(C)]
    struct VkPipelineVertexInputStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, vertex_binding_description_count: u32,
        p_vertex_binding_descriptions: *const VkVertexInputBindingDescription,
        vertex_attribute_description_count: u32, p_vertex_attribute_descriptions: *const VkVertexInputAttributeDescription,
    }
    #[repr(C)]
    struct VkPipelineInputAssemblyStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, topology: i32, primitive_restart_enable: VkBool32,
    }
    #[repr(C)]
    struct VkPipelineViewportStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, viewport_count: u32, p_viewports: *const VkViewport,
        scissor_count: u32, p_scissors: *const VkRect2D,
    }
    #[repr(C)]
    struct VkPipelineRasterizationStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, depth_clamp_enable: VkBool32, rasterizer_discard_enable: VkBool32,
        polygon_mode: i32, cull_mode: VkFlags, front_face: i32, depth_bias_enable: VkBool32,
        depth_bias_constant_factor: f32, depth_bias_clamp: f32, depth_bias_slope_factor: f32, line_width: f32,
    }
    #[repr(C)]
    struct VkPipelineMultisampleStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, rasterization_samples: u32, sample_shading_enable: VkBool32,
        min_sample_shading: f32, p_sample_mask: *const u32, alpha_to_coverage_enable: VkBool32, alpha_to_one_enable: VkBool32,
    }
    #[repr(C)]
    struct VkPipelineColorBlendAttachmentState {
        blend_enable: VkBool32, src_color_blend_factor: i32, dst_color_blend_factor: i32, color_blend_op: i32,
        src_alpha_blend_factor: i32, dst_alpha_blend_factor: i32, alpha_blend_op: i32, color_write_mask: VkFlags,
    }
    #[repr(C)]
    struct VkPipelineColorBlendStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, logic_op_enable: VkBool32, logic_op: i32,
        attachment_count: u32, p_attachments: *const VkPipelineColorBlendAttachmentState, blend_constants: [f32; 4],
    }
    #[repr(C)]
    struct VkPipelineDynamicStateCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, dynamic_state_count: u32, p_dynamic_states: *const i32,
    }
    #[repr(C)]
    struct VkGraphicsPipelineCreateInfo {
        s_type: i32, p_next: *const c_void, flags: VkFlags, stage_count: u32, p_stages: *const VkPipelineShaderStageCreateInfo,
        p_vertex_input_state: *const VkPipelineVertexInputStateCreateInfo,
        p_input_assembly_state: *const VkPipelineInputAssemblyStateCreateInfo,
        p_tessellation_state: *const c_void,
        p_viewport_state: *const VkPipelineViewportStateCreateInfo,
        p_rasterization_state: *const VkPipelineRasterizationStateCreateInfo,
        p_multisample_state: *const VkPipelineMultisampleStateCreateInfo,
        p_depth_stencil_state: *const c_void,
        p_color_blend_state: *const VkPipelineColorBlendStateCreateInfo,
        p_dynamic_state: *const VkPipelineDynamicStateCreateInfo,
        layout: VkPipelineLayout, render_pass: VkRenderPass, subpass: u32,
        base_pipeline_handle: VkPipeline, base_pipeline_index: i32,
    }
    #[repr(C)]
    struct VkCommandPoolCreateInfo { s_type: i32, p_next: *const c_void, flags: VkFlags, queue_family_index: u32 }
    #[repr(C)]
    struct VkCommandBufferAllocateInfo { s_type: i32, p_next: *const c_void, command_pool: VkCommandPool, level: i32, command_buffer_count: u32 }
    #[repr(C)]
    struct VkCommandBufferBeginInfo { s_type: i32, p_next: *const c_void, flags: VkFlags, p_inheritance_info: *const c_void }
    #[repr(C)]
    struct VkRenderPassBeginInfo {
        s_type: i32, p_next: *const c_void, render_pass: VkRenderPass, framebuffer: VkFramebuffer,
        render_area: VkRect2D, clear_value_count: u32, p_clear_values: *const c_void,
    }
    #[repr(C)]
    struct VkImageMemoryBarrier {
        s_type: i32, p_next: *const c_void, src_access_mask: VkFlags, dst_access_mask: VkFlags,
        old_layout: i32, new_layout: i32, src_queue_family_index: u32, dst_queue_family_index: u32,
        image: VkImage, subresource_range: VkImageSubresourceRange,
    }
    #[repr(C)]
    struct VkImageSubresourceLayers { aspect_mask: VkFlags, mip_level: u32, base_array_layer: u32, layer_count: u32 }
    #[repr(C)]
    struct VkBufferImageCopy {
        buffer_offset: VkDeviceSize, buffer_row_length: u32, buffer_image_height: u32,
        image_subresource: VkImageSubresourceLayers, image_offset: VkOffset3D, image_extent: VkExtent3D,
    }
    #[repr(C)]
    struct VkSubmitInfo {
        s_type: i32, p_next: *const c_void, wait_semaphore_count: u32, p_wait_semaphores: *const VkSemaphore,
        p_wait_dst_stage_mask: *const VkFlags, command_buffer_count: u32, p_command_buffers: *const VkCommandBuffer,
        signal_semaphore_count: u32, p_signal_semaphores: *const VkSemaphore,
    }
    #[repr(C)]
    struct VkFenceCreateInfo { s_type: i32, p_next: *const c_void, flags: VkFlags }
    #[repr(C)]
    struct VkSemaphoreCreateInfo { s_type: i32, p_next: *const c_void, flags: VkFlags }

    // ---- Function pointer types -----------------------------------------

    type PfnGetInstanceProcAddr = unsafe extern "system" fn(VkInstance, *const c_char) -> *mut c_void;
    type PfnGetDeviceProcAddr = unsafe extern "system" fn(VkDevice, *const c_char) -> *mut c_void;
    type PfnCreateWin32SurfaceKHR = unsafe extern "system" fn(VkInstance, *const VkWin32SurfaceCreateInfoKHR, *const c_void, *mut VkSurfaceKHR) -> VkResult;
    type PfnDestroySurfaceKHR = unsafe extern "system" fn(VkInstance, VkSurfaceKHR, *const c_void);
    type PfnCreateDevice = unsafe extern "system" fn(VkPhysicalDevice, *const VkDeviceCreateInfo, *const c_void, *mut VkDevice) -> VkResult;
    type PfnDestroyDevice = unsafe extern "system" fn(VkDevice, *const c_void);
    type PfnCreateSwapchainKHR = unsafe extern "system" fn(VkDevice, *const VkSwapchainCreateInfoKHR, *const c_void, *mut VkSwapchainKHR) -> VkResult;
    type PfnDestroySwapchainKHR = unsafe extern "system" fn(VkDevice, VkSwapchainKHR, *const c_void);
    type PfnQueuePresentKHR = unsafe extern "system" fn(VkQueue, *const VkPresentInfoKHR) -> VkResult;

    static REAL_GIPA: AtomicUsize = AtomicUsize::new(0);
    static REAL_GDPA: AtomicUsize = AtomicUsize::new(0);
    static REAL_CREATE_WIN32_SURFACE: AtomicUsize = AtomicUsize::new(0);
    static REAL_DESTROY_SURFACE: AtomicUsize = AtomicUsize::new(0);
    static REAL_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
    static REAL_DESTROY_DEVICE: AtomicUsize = AtomicUsize::new(0);
    static REAL_CREATE_SWAPCHAIN: AtomicUsize = AtomicUsize::new(0);
    static REAL_DESTROY_SWAPCHAIN: AtomicUsize = AtomicUsize::new(0);
    static REAL_QUEUE_PRESENT: AtomicUsize = AtomicUsize::new(0);
    static REAL_GET_MEM_PROPS: AtomicUsize = AtomicUsize::new(0);

    // ---- Device dispatch table ------------------------------------------

    #[derive(Clone, Copy)]
    struct VkDevFns {
        get_swapchain_images: unsafe extern "system" fn(VkDevice, VkSwapchainKHR, *mut u32, *mut VkImage) -> VkResult,
        create_image_view: unsafe extern "system" fn(VkDevice, *const VkImageViewCreateInfo, *const c_void, *mut VkImageView) -> VkResult,
        destroy_image_view: unsafe extern "system" fn(VkDevice, VkImageView, *const c_void),
        create_shader_module: unsafe extern "system" fn(VkDevice, *const VkShaderModuleCreateInfo, *const c_void, *mut VkShaderModule) -> VkResult,
        destroy_shader_module: unsafe extern "system" fn(VkDevice, VkShaderModule, *const c_void),
        create_render_pass: unsafe extern "system" fn(VkDevice, *const VkRenderPassCreateInfo, *const c_void, *mut VkRenderPass) -> VkResult,
        destroy_render_pass: unsafe extern "system" fn(VkDevice, VkRenderPass, *const c_void),
        create_framebuffer: unsafe extern "system" fn(VkDevice, *const VkFramebufferCreateInfo, *const c_void, *mut VkFramebuffer) -> VkResult,
        destroy_framebuffer: unsafe extern "system" fn(VkDevice, VkFramebuffer, *const c_void),
        create_descriptor_set_layout: unsafe extern "system" fn(VkDevice, *const VkDescriptorSetLayoutCreateInfo, *const c_void, *mut VkDescriptorSetLayout) -> VkResult,
        destroy_descriptor_set_layout: unsafe extern "system" fn(VkDevice, VkDescriptorSetLayout, *const c_void),
        create_descriptor_pool: unsafe extern "system" fn(VkDevice, *const VkDescriptorPoolCreateInfo, *const c_void, *mut VkDescriptorPool) -> VkResult,
        destroy_descriptor_pool: unsafe extern "system" fn(VkDevice, VkDescriptorPool, *const c_void),
        allocate_descriptor_sets: unsafe extern "system" fn(VkDevice, *const VkDescriptorSetAllocateInfo, *mut VkDescriptorSet) -> VkResult,
        free_descriptor_sets: unsafe extern "system" fn(VkDevice, VkDescriptorPool, u32, *const VkDescriptorSet) -> VkResult,
        update_descriptor_sets: unsafe extern "system" fn(VkDevice, u32, *const VkWriteDescriptorSet, u32, *const c_void),
        create_sampler: unsafe extern "system" fn(VkDevice, *const VkSamplerCreateInfo, *const c_void, *mut VkSampler) -> VkResult,
        destroy_sampler: unsafe extern "system" fn(VkDevice, VkSampler, *const c_void),
        create_pipeline_layout: unsafe extern "system" fn(VkDevice, *const VkPipelineLayoutCreateInfo, *const c_void, *mut VkPipelineLayout) -> VkResult,
        destroy_pipeline_layout: unsafe extern "system" fn(VkDevice, VkPipelineLayout, *const c_void),
        create_graphics_pipelines: unsafe extern "system" fn(VkDevice, u64, u32, *const VkGraphicsPipelineCreateInfo, *const c_void, *mut VkPipeline) -> VkResult,
        destroy_pipeline: unsafe extern "system" fn(VkDevice, VkPipeline, *const c_void),
        create_command_pool: unsafe extern "system" fn(VkDevice, *const VkCommandPoolCreateInfo, *const c_void, *mut VkCommandPool) -> VkResult,
        destroy_command_pool: unsafe extern "system" fn(VkDevice, VkCommandPool, *const c_void),
        allocate_command_buffers: unsafe extern "system" fn(VkDevice, *const VkCommandBufferAllocateInfo, *mut VkCommandBuffer) -> VkResult,
        begin_command_buffer: unsafe extern "system" fn(VkCommandBuffer, *const VkCommandBufferBeginInfo) -> VkResult,
        end_command_buffer: unsafe extern "system" fn(VkCommandBuffer) -> VkResult,
        reset_command_buffer: unsafe extern "system" fn(VkCommandBuffer, VkFlags) -> VkResult,
        create_fence: unsafe extern "system" fn(VkDevice, *const VkFenceCreateInfo, *const c_void, *mut VkFence) -> VkResult,
        destroy_fence: unsafe extern "system" fn(VkDevice, VkFence, *const c_void),
        wait_for_fences: unsafe extern "system" fn(VkDevice, u32, *const VkFence, VkBool32, u64) -> VkResult,
        reset_fences: unsafe extern "system" fn(VkDevice, u32, *const VkFence) -> VkResult,
        create_semaphore: unsafe extern "system" fn(VkDevice, *const VkSemaphoreCreateInfo, *const c_void, *mut VkSemaphore) -> VkResult,
        destroy_semaphore: unsafe extern "system" fn(VkDevice, VkSemaphore, *const c_void),
        create_buffer: unsafe extern "system" fn(VkDevice, *const VkBufferCreateInfo, *const c_void, *mut VkBuffer) -> VkResult,
        destroy_buffer: unsafe extern "system" fn(VkDevice, VkBuffer, *const c_void),
        get_buffer_memory_requirements: unsafe extern "system" fn(VkDevice, VkBuffer, *mut VkMemoryRequirements),
        bind_buffer_memory: unsafe extern "system" fn(VkDevice, VkBuffer, VkDeviceMemory, VkDeviceSize) -> VkResult,
        create_image: unsafe extern "system" fn(VkDevice, *const VkImageCreateInfo, *const c_void, *mut VkImage) -> VkResult,
        destroy_image: unsafe extern "system" fn(VkDevice, VkImage, *const c_void),
        get_image_memory_requirements: unsafe extern "system" fn(VkDevice, VkImage, *mut VkMemoryRequirements),
        bind_image_memory: unsafe extern "system" fn(VkDevice, VkImage, VkDeviceMemory, VkDeviceSize) -> VkResult,
        allocate_memory: unsafe extern "system" fn(VkDevice, *const VkMemoryAllocateInfo, *const c_void, *mut VkDeviceMemory) -> VkResult,
        free_memory: unsafe extern "system" fn(VkDevice, VkDeviceMemory, *const c_void),
        map_memory: unsafe extern "system" fn(VkDevice, VkDeviceMemory, VkDeviceSize, VkDeviceSize, VkFlags, *mut *mut c_void) -> VkResult,
        unmap_memory: unsafe extern "system" fn(VkDevice, VkDeviceMemory),
        queue_submit: unsafe extern "system" fn(VkQueue, u32, *const VkSubmitInfo, VkFence) -> VkResult,
        device_wait_idle: unsafe extern "system" fn(VkDevice) -> VkResult,
        cmd_pipeline_barrier: unsafe extern "system" fn(VkCommandBuffer, VkFlags, VkFlags, VkFlags, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier),
        cmd_copy_buffer_to_image: unsafe extern "system" fn(VkCommandBuffer, VkBuffer, VkImage, i32, u32, *const VkBufferImageCopy),
        cmd_begin_render_pass: unsafe extern "system" fn(VkCommandBuffer, *const VkRenderPassBeginInfo, i32),
        cmd_end_render_pass: unsafe extern "system" fn(VkCommandBuffer),
        cmd_bind_pipeline: unsafe extern "system" fn(VkCommandBuffer, i32, VkPipeline),
        cmd_set_viewport: unsafe extern "system" fn(VkCommandBuffer, u32, u32, *const VkViewport),
        cmd_set_scissor: unsafe extern "system" fn(VkCommandBuffer, u32, u32, *const VkRect2D),
        cmd_bind_descriptor_sets: unsafe extern "system" fn(VkCommandBuffer, i32, VkPipelineLayout, u32, u32, *const VkDescriptorSet, u32, *const u32),
        cmd_bind_vertex_buffers: unsafe extern "system" fn(VkCommandBuffer, u32, u32, *const VkBuffer, *const VkDeviceSize),
        cmd_bind_index_buffer: unsafe extern "system" fn(VkCommandBuffer, VkBuffer, VkDeviceSize, i32),
        cmd_push_constants: unsafe extern "system" fn(VkCommandBuffer, VkPipelineLayout, VkFlags, u32, u32, *const c_void),
        cmd_draw_indexed: unsafe extern "system" fn(VkCommandBuffer, u32, u32, u32, i32, u32),
    }

    impl VkDevFns {
        unsafe fn load(device: VkDevice) -> Option<Self> {
            let gdpa_raw = REAL_GDPA.load(Ordering::Acquire);
            if gdpa_raw == 0 {
                return None;
            }
            let gdpa: PfnGetDeviceProcAddr = unsafe { std::mem::transmute(gdpa_raw) };
            macro_rules! sym {
                ($name:literal) => {{
                    let p = unsafe { gdpa(device, concat!($name, "\0").as_ptr() as *const c_char) };
                    if p.is_null() { return None; }
                    unsafe { std::mem::transmute(p) }
                }};
            }
            Some(Self {
                get_swapchain_images: sym!("vkGetSwapchainImagesKHR"),
                create_image_view: sym!("vkCreateImageView"),
                destroy_image_view: sym!("vkDestroyImageView"),
                create_shader_module: sym!("vkCreateShaderModule"),
                destroy_shader_module: sym!("vkDestroyShaderModule"),
                create_render_pass: sym!("vkCreateRenderPass"),
                destroy_render_pass: sym!("vkDestroyRenderPass"),
                create_framebuffer: sym!("vkCreateFramebuffer"),
                destroy_framebuffer: sym!("vkDestroyFramebuffer"),
                create_descriptor_set_layout: sym!("vkCreateDescriptorSetLayout"),
                destroy_descriptor_set_layout: sym!("vkDestroyDescriptorSetLayout"),
                create_descriptor_pool: sym!("vkCreateDescriptorPool"),
                destroy_descriptor_pool: sym!("vkDestroyDescriptorPool"),
                allocate_descriptor_sets: sym!("vkAllocateDescriptorSets"),
                free_descriptor_sets: sym!("vkFreeDescriptorSets"),
                update_descriptor_sets: sym!("vkUpdateDescriptorSets"),
                create_sampler: sym!("vkCreateSampler"),
                destroy_sampler: sym!("vkDestroySampler"),
                create_pipeline_layout: sym!("vkCreatePipelineLayout"),
                destroy_pipeline_layout: sym!("vkDestroyPipelineLayout"),
                create_graphics_pipelines: sym!("vkCreateGraphicsPipelines"),
                destroy_pipeline: sym!("vkDestroyPipeline"),
                create_command_pool: sym!("vkCreateCommandPool"),
                destroy_command_pool: sym!("vkDestroyCommandPool"),
                allocate_command_buffers: sym!("vkAllocateCommandBuffers"),
                begin_command_buffer: sym!("vkBeginCommandBuffer"),
                end_command_buffer: sym!("vkEndCommandBuffer"),
                reset_command_buffer: sym!("vkResetCommandBuffer"),
                create_fence: sym!("vkCreateFence"),
                destroy_fence: sym!("vkDestroyFence"),
                wait_for_fences: sym!("vkWaitForFences"),
                reset_fences: sym!("vkResetFences"),
                create_semaphore: sym!("vkCreateSemaphore"),
                destroy_semaphore: sym!("vkDestroySemaphore"),
                create_buffer: sym!("vkCreateBuffer"),
                destroy_buffer: sym!("vkDestroyBuffer"),
                get_buffer_memory_requirements: sym!("vkGetBufferMemoryRequirements"),
                bind_buffer_memory: sym!("vkBindBufferMemory"),
                create_image: sym!("vkCreateImage"),
                destroy_image: sym!("vkDestroyImage"),
                get_image_memory_requirements: sym!("vkGetImageMemoryRequirements"),
                bind_image_memory: sym!("vkBindImageMemory"),
                allocate_memory: sym!("vkAllocateMemory"),
                free_memory: sym!("vkFreeMemory"),
                map_memory: sym!("vkMapMemory"),
                unmap_memory: sym!("vkUnmapMemory"),
                queue_submit: sym!("vkQueueSubmit"),
                device_wait_idle: sym!("vkDeviceWaitIdle"),
                cmd_pipeline_barrier: sym!("vkCmdPipelineBarrier"),
                cmd_copy_buffer_to_image: sym!("vkCmdCopyBufferToImage"),
                cmd_begin_render_pass: sym!("vkCmdBeginRenderPass"),
                cmd_end_render_pass: sym!("vkCmdEndRenderPass"),
                cmd_bind_pipeline: sym!("vkCmdBindPipeline"),
                cmd_set_viewport: sym!("vkCmdSetViewport"),
                cmd_set_scissor: sym!("vkCmdSetScissor"),
                cmd_bind_descriptor_sets: sym!("vkCmdBindDescriptorSets"),
                cmd_bind_vertex_buffers: sym!("vkCmdBindVertexBuffers"),
                cmd_bind_index_buffer: sym!("vkCmdBindIndexBuffer"),
                cmd_push_constants: sym!("vkCmdPushConstants"),
                cmd_draw_indexed: sym!("vkCmdDrawIndexed"),
            })
        }
    }

    // ---- Tracked Vulkan objects -----------------------------------------

    struct DeviceMeta {
        queue_family: u32,
        mem_props: VkPhysicalDeviceMemoryProperties,
        fns: VkDevFns,
    }

    struct VkBuf {
        buf: VkBuffer,
        mem: VkDeviceMemory,
        mapped: *mut u8,
        cap: usize,
    }

    struct VkTex {
        image: VkImage,
        mem: VkDeviceMemory,
        view: VkImageView,
        set: VkDescriptorSet,
        size: [u32; 2],
        linear: bool,
    }

    struct VkFrameSlot {
        view: VkImageView,
        fb: VkFramebuffer,
        cmd: VkCommandBuffer,
        fence: VkFence,
        done_sem: VkSemaphore,
        vb: Option<VkBuf>,
        ib: Option<VkBuf>,
        staging: Vec<(VkBuffer, VkDeviceMemory)>,
        last_submit_gen: u64,
        fence_pending: bool,
    }

    struct VkSwapchainRenderer {
        device: VkDevice,
        fns: VkDevFns,
        mem_props: VkPhysicalDeviceMemoryProperties,
        hwnd: HWND,
        size: (u32, u32),
        linear_out: bool,
        render_pass: VkRenderPass,
        ds_layout: VkDescriptorSetLayout,
        ds_pool: VkDescriptorPool,
        sampler_lin: VkSampler,
        sampler_near: VkSampler,
        pipe_layout: VkPipelineLayout,
        pipeline: VkPipeline,
        cmd_pool: VkCommandPool,
        slots: Vec<VkFrameSlot>,
        textures: HashMap<u64, VkTex>,
        submit_gen: u64,
        retired_tex: Vec<(u64, VkTex)>,
    }

    unsafe impl Send for VkSwapchainRenderer {}
    unsafe impl Send for DeviceMeta {}

    static SURFACES: std::sync::LazyLock<Mutex<HashMap<VkSurfaceKHR, isize>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    static DEVICES: std::sync::LazyLock<Mutex<HashMap<usize, DeviceMeta>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    static SWAPCHAINS: std::sync::LazyLock<Mutex<HashMap<VkSwapchainKHR, VkSwapchainRenderer>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

    // ---- Hook installation (W^X-safe IAT) -------------------------------

    pub fn install_if_loaded() -> bool {
        unsafe {
            use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
            let Ok(vk) = GetModuleHandleW(w!("vulkan-1.dll")) else {
                return false;
            };
            if let Some(p) = GetProcAddress(vk, s!("vkGetPhysicalDeviceMemoryProperties")) {
                REAL_GET_MEM_PROPS.compare_exchange(0, p as usize, Ordering::AcqRel, Ordering::Acquire).ok();
            }
            let pairs: &[(&windows::core::PCSTR, &AtomicUsize, usize)] = &[
                (&s!("vkGetInstanceProcAddr"), &REAL_GIPA, hook_gipa as *const () as usize),
                (&s!("vkGetDeviceProcAddr"), &REAL_GDPA, hook_gdpa as *const () as usize),
                (&s!("vkCreateWin32SurfaceKHR"), &REAL_CREATE_WIN32_SURFACE, hook_create_win32_surface as *const () as usize),
                (&s!("vkDestroySurfaceKHR"), &REAL_DESTROY_SURFACE, hook_destroy_surface as *const () as usize),
                (&s!("vkCreateDevice"), &REAL_CREATE_DEVICE, hook_create_device as *const () as usize),
                (&s!("vkDestroyDevice"), &REAL_DESTROY_DEVICE, hook_destroy_device as *const () as usize),
                (&s!("vkCreateSwapchainKHR"), &REAL_CREATE_SWAPCHAIN, hook_create_swapchain as *const () as usize),
                (&s!("vkDestroySwapchainKHR"), &REAL_DESTROY_SWAPCHAIN, hook_destroy_swapchain as *const () as usize),
                (&s!("vkQueuePresentKHR"), &REAL_QUEUE_PRESENT, hook_queue_present as *const () as usize),
            ];
            let mut table = Vec::new();
            for (name, slot, ours) in pairs {
                if let Some(p) = GetProcAddress(vk, **name) {
                    slot.compare_exchange(0, p as usize, Ordering::AcqRel, Ordering::Acquire).ok();
                    table.push((slot.load(Ordering::Acquire), *ours));
                }
            }
            let patched = util::patch_all_modules(&table, &[vk]);
            if patched > 0 {
                crate::log!("overlay: Vulkan hooks in ({patched} IAT slots)");
            }
            patched > 0 || REAL_GIPA.load(Ordering::Acquire) != 0
        }
    }

    unsafe extern "system" fn hook_gipa(instance: VkInstance, name: *const c_char) -> *mut c_void {
        let real: PfnGetInstanceProcAddr = unsafe { std::mem::transmute(REAL_GIPA.load(Ordering::Acquire)) };
        let ptr = unsafe { real(instance, name) };
        if ptr.is_null() || name.is_null() {
            return ptr;
        }
        let s = unsafe { CStr::from_ptr(name) }.to_bytes();
        match s {
            b"vkGetInstanceProcAddr" => {
                REAL_GIPA.compare_exchange(0, ptr as usize, Ordering::AcqRel, Ordering::Acquire).ok();
                hook_gipa as *mut c_void
            }
            b"vkGetDeviceProcAddr" => {
                REAL_GDPA.compare_exchange(0, ptr as usize, Ordering::AcqRel, Ordering::Acquire).ok();
                hook_gdpa as *mut c_void
            }
            b"vkCreateWin32SurfaceKHR" => {
                REAL_CREATE_WIN32_SURFACE.store(ptr as usize, Ordering::Release);
                hook_create_win32_surface as *mut c_void
            }
            b"vkDestroySurfaceKHR" => {
                REAL_DESTROY_SURFACE.store(ptr as usize, Ordering::Release);
                hook_destroy_surface as *mut c_void
            }
            b"vkCreateDevice" => {
                REAL_CREATE_DEVICE.store(ptr as usize, Ordering::Release);
                hook_create_device as *mut c_void
            }
            b"vkDestroyDevice" => {
                REAL_DESTROY_DEVICE.store(ptr as usize, Ordering::Release);
                hook_destroy_device as *mut c_void
            }
            b"vkCreateSwapchainKHR" => {
                REAL_CREATE_SWAPCHAIN.store(ptr as usize, Ordering::Release);
                hook_create_swapchain as *mut c_void
            }
            b"vkDestroySwapchainKHR" => {
                REAL_DESTROY_SWAPCHAIN.store(ptr as usize, Ordering::Release);
                hook_destroy_swapchain as *mut c_void
            }
            b"vkQueuePresentKHR" => {
                REAL_QUEUE_PRESENT.store(ptr as usize, Ordering::Release);
                hook_queue_present as *mut c_void
            }
            _ => ptr,
        }
    }

    unsafe extern "system" fn hook_gdpa(device: VkDevice, name: *const c_char) -> *mut c_void {
        let real: PfnGetDeviceProcAddr = unsafe { std::mem::transmute(REAL_GDPA.load(Ordering::Acquire)) };
        let ptr = unsafe { real(device, name) };
        if ptr.is_null() || name.is_null() {
            return ptr;
        }
        let s = unsafe { CStr::from_ptr(name) }.to_bytes();
        match s {
            b"vkDestroyDevice" => {
                REAL_DESTROY_DEVICE.store(ptr as usize, Ordering::Release);
                hook_destroy_device as *mut c_void
            }
            b"vkCreateSwapchainKHR" => {
                REAL_CREATE_SWAPCHAIN.store(ptr as usize, Ordering::Release);
                hook_create_swapchain as *mut c_void
            }
            b"vkDestroySwapchainKHR" => {
                REAL_DESTROY_SWAPCHAIN.store(ptr as usize, Ordering::Release);
                hook_destroy_swapchain as *mut c_void
            }
            b"vkQueuePresentKHR" => {
                REAL_QUEUE_PRESENT.store(ptr as usize, Ordering::Release);
                hook_queue_present as *mut c_void
            }
            _ => ptr,
        }
    }

    unsafe extern "system" fn hook_create_win32_surface(
        instance: VkInstance,
        info: *const VkWin32SurfaceCreateInfoKHR,
        alloc: *const c_void,
        surface: *mut VkSurfaceKHR,
    ) -> VkResult {
        let real: PfnCreateWin32SurfaceKHR = unsafe { std::mem::transmute(REAL_CREATE_WIN32_SURFACE.load(Ordering::Acquire)) };
        let res = unsafe { real(instance, info, alloc, surface) };
        if res == VK_SUCCESS && !info.is_null() && !surface.is_null() {
            let hwnd = unsafe { (*info).hwnd.0 as isize };
            let surf = unsafe { *surface };
            if let Ok(mut map) = SURFACES.lock() {
                map.insert(surf, hwnd);
            }
        }
        res
    }

    unsafe extern "system" fn hook_destroy_surface(instance: VkInstance, surface: VkSurfaceKHR, alloc: *const c_void) {
        if let Ok(mut map) = SURFACES.lock() {
            map.remove(&surface);
        }
        let real: PfnDestroySurfaceKHR = unsafe { std::mem::transmute(REAL_DESTROY_SURFACE.load(Ordering::Acquire)) };
        unsafe { real(instance, surface, alloc) }
    }

    unsafe extern "system" fn hook_create_device(
        phys: VkPhysicalDevice,
        info: *const VkDeviceCreateInfo,
        alloc: *const c_void,
        device: *mut VkDevice,
    ) -> VkResult {
        let real: PfnCreateDevice = unsafe { std::mem::transmute(REAL_CREATE_DEVICE.load(Ordering::Acquire)) };
        let res = unsafe { real(phys, info, alloc, device) };
        if res == VK_SUCCESS && !device.is_null() && !info.is_null() {
            let _ = std::panic::catch_unwind(|| unsafe {
                let dev = *device;
                let queue_family = if (*info).queue_create_info_count > 0 && !(*info).p_queue_create_infos.is_null() {
                    (*(*info).p_queue_create_infos).queue_family_index
                } else {
                    0
                };
                let mut mem_props = VkPhysicalDeviceMemoryProperties {
                    memory_type_count: 0,
                    memory_types: [VkMemoryType::default(); 32],
                    memory_heap_count: 0,
                    memory_heaps: [VkMemoryHeap::default(); 16],
                };
                let gmp = REAL_GET_MEM_PROPS.load(Ordering::Acquire);
                if gmp != 0 {
                    type PfnGetMemProps = unsafe extern "system" fn(VkPhysicalDevice, *mut VkPhysicalDeviceMemoryProperties);
                    let f: PfnGetMemProps = std::mem::transmute(gmp);
                    f(phys, &mut mem_props);
                }
                if let Some(fns) = VkDevFns::load(dev) {
                    if let Ok(mut map) = DEVICES.lock() {
                        map.insert(dev as usize, DeviceMeta { queue_family, mem_props, fns });
                    }
                }
            });
        }
        res
    }

    unsafe extern "system" fn hook_destroy_device(device: VkDevice, alloc: *const c_void) {
        let _ = std::panic::catch_unwind(|| {
            if let Ok(mut scs) = SWAPCHAINS.lock() {
                let keys: Vec<VkSwapchainKHR> = scs.iter().filter(|(_, r)| r.device == device).map(|(&k, _)| k).collect();
                for k in keys {
                    if let Some(mut r) = scs.remove(&k) {
                        unsafe { r.destroy() };
                    }
                }
            }
            if let Ok(mut devs) = DEVICES.lock() {
                devs.remove(&(device as usize));
            }
        });
        let real: PfnDestroyDevice = unsafe { std::mem::transmute(REAL_DESTROY_DEVICE.load(Ordering::Acquire)) };
        unsafe { real(device, alloc) }
    }

    unsafe extern "system" fn hook_create_swapchain(
        device: VkDevice,
        info: *const VkSwapchainCreateInfoKHR,
        alloc: *const c_void,
        swapchain: *mut VkSwapchainKHR,
    ) -> VkResult {
        let real: PfnCreateSwapchainKHR = unsafe { std::mem::transmute(REAL_CREATE_SWAPCHAIN.load(Ordering::Acquire)) };
        let res = unsafe { real(device, info, alloc, swapchain) };
        if res == VK_SUCCESS && !info.is_null() && !swapchain.is_null() {
            let _ = std::panic::catch_unwind(|| unsafe {
                let sc = *swapchain;
                let ci = &*info;
                let hwnd = SURFACES.lock().ok().and_then(|m| m.get(&ci.surface).copied()).map(|h| HWND(h as *mut c_void)).unwrap_or_default();
                let meta = DEVICES.lock().ok().and_then(|m| m.get(&(device as usize)).map(|d| (d.queue_family, d.mem_props, d.fns)));
                if let Some((qf, mem_props, fns)) = meta {
                    if let Some(r) = VkSwapchainRenderer::new(device, fns, mem_props, qf, sc, hwnd, ci.image_format, ci.image_extent) {
                        if let Ok(mut scs) = SWAPCHAINS.lock() {
                            scs.insert(sc, r);
                        }
                    }
                }
            });
        }
        res
    }

    unsafe extern "system" fn hook_destroy_swapchain(device: VkDevice, swapchain: VkSwapchainKHR, alloc: *const c_void) {
        let _ = std::panic::catch_unwind(|| {
            if let Ok(mut scs) = SWAPCHAINS.lock() {
                if let Some(mut r) = scs.remove(&swapchain) {
                    unsafe { r.destroy() };
                }
            }
        });
        let real: PfnDestroySwapchainKHR = unsafe { std::mem::transmute(REAL_DESTROY_SWAPCHAIN.load(Ordering::Acquire)) };
        unsafe { real(device, swapchain, alloc) }
    }

    unsafe extern "system" fn hook_queue_present(queue: VkQueue, info: *const VkPresentInfoKHR) -> VkResult {
        let real: PfnQueuePresentKHR = unsafe { std::mem::transmute(REAL_QUEUE_PRESENT.load(Ordering::Acquire)) };
        if info.is_null() {
            return unsafe { real(queue, info) };
        }
        let modified = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let active = util::active_backend();
            if active != Backend::None && active != Backend::Vulkan {
                return None;
            }
            let pi = &*info;
            if pi.swapchain_count == 0 || pi.p_swapchains.is_null() || pi.p_image_indices.is_null() {
                return None;
            }
            let sc = *pi.p_swapchains;
            let img_idx = *pi.p_image_indices as usize;
            let Ok(mut scs) = SWAPCHAINS.lock() else { return None };
            let Some(r) = scs.get_mut(&sc) else { return None };

            if active == Backend::None {
                if !util::claim_backend(Backend::Vulkan) {
                    return None;
                }
                crate::log!("overlay: drawing into the game's Vulkan swapchain ({}x{})", r.size.0, r.size.1);
                if !r.hwnd.is_invalid() {
                    crate::overlay::input::attach(r.hwnd);
                }
                crate::overlay::hooked(r.size);
            } else {
                crate::overlay::resized(r.size);
            }

            let (ops, frame) = shared::take_for_present();
            let has_draw = frame.as_ref().is_some_and(|f| !f.meshes.is_empty());
            if ops.is_empty() && !has_draw {
                return None;
            }
            let wait_sems = if pi.wait_semaphore_count > 0 && !pi.p_wait_semaphores.is_null() {
                std::slice::from_raw_parts(pi.p_wait_semaphores, pi.wait_semaphore_count as usize)
            } else {
                &[]
            };
            if let Some(done_sem) = r.render(queue, img_idx, wait_sems, ops, frame.as_deref()) {
                let mut next_pi = *pi;
                next_pi.wait_semaphore_count = 1;
                return Some((next_pi, done_sem));
            }
            None
        }))
        .ok()
        .flatten();
        if let Some((mut next_pi, done_sem)) = modified {
            next_pi.p_wait_semaphores = &done_sem;
            return unsafe { real(queue, &next_pi) };
        }
        unsafe { real(queue, info) }
    }

    // ---- Vulkan Renderer ------------------------------------------------

    impl VkSwapchainRenderer {
        fn find_mem_type(&self, type_bits: u32, flags: VkFlags) -> Option<u32> {
            for i in 0..self.mem_props.memory_type_count.min(32) {
                if (type_bits & (1 << i)) != 0 && (self.mem_props.memory_types[i as usize].property_flags & flags) == flags {
                    return Some(i);
                }
            }
            None
        }

        unsafe fn new(
            device: VkDevice,
            fns: VkDevFns,
            mem_props: VkPhysicalDeviceMemoryProperties,
            queue_family: u32,
            swapchain: VkSwapchainKHR,
            hwnd: HWND,
            format: i32,
            extent: VkExtent2D,
        ) -> Option<Self> {
            if extent.width == 0 || extent.height == 0 {
                return None;
            }
            unsafe {
                let mut count = 0u32;
                if (fns.get_swapchain_images)(device, swapchain, &mut count, std::ptr::null_mut()) != VK_SUCCESS || count == 0 {
                    return None;
                }
                let mut images = vec![0u64; count as usize];
                if (fns.get_swapchain_images)(device, swapchain, &mut count, images.as_mut_ptr()) != VK_SUCCESS {
                    return None;
                }

                // Render pass: LOAD existing swapchain contents, keep PRESENT_SRC_KHR layout.
                let att = VkAttachmentDescription {
                    flags: 0,
                    format,
                    samples: 1,
                    load_op: 0,  // LOAD
                    store_op: 0, // STORE
                    stencil_load_op: 2,
                    stencil_store_op: 1,
                    initial_layout: VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
                    final_layout: VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
                };
                let color_ref = VkAttachmentReference { attachment: 0, layout: VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL };
                let subpass = VkSubpassDescription {
                    flags: 0,
                    pipeline_bind_point: 0,
                    input_attachment_count: 0,
                    p_input_attachments: std::ptr::null(),
                    color_attachment_count: 1,
                    p_color_attachments: &color_ref,
                    p_resolve_attachments: std::ptr::null(),
                    p_depth_stencil_attachment: std::ptr::null(),
                    preserve_attachment_count: 0,
                    p_preserve_attachments: std::ptr::null(),
                };
                let dep = VkSubpassDependency {
                    src_subpass: u32::MAX, // VK_SUBPASS_EXTERNAL
                    dst_subpass: 0,
                    src_stage_mask: VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
                    dst_stage_mask: VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
                    src_access_mask: 0,
                    dst_access_mask: VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
                    dependency_flags: 0,
                };
                let rp_ci = VkRenderPassCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    attachment_count: 1,
                    p_attachments: &att,
                    subpass_count: 1,
                    p_subpasses: &subpass,
                    dependency_count: 1,
                    p_dependencies: &dep,
                };
                let mut render_pass = 0;
                if (fns.create_render_pass)(device, &rp_ci, std::ptr::null(), &mut render_pass) != VK_SUCCESS {
                    return None;
                }

                // Descriptor set layout + pool + samplers.
                let binding = VkDescriptorSetLayoutBinding {
                    binding: 0,
                    descriptor_type: VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
                    descriptor_count: 1,
                    stage_flags: VK_SHADER_STAGE_FRAGMENT_BIT,
                    p_immutable_samplers: std::ptr::null(),
                };
                let dsl_ci = VkDescriptorSetLayoutCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    binding_count: 1,
                    p_bindings: &binding,
                };
                let mut ds_layout = 0;
                if (fns.create_descriptor_set_layout)(device, &dsl_ci, std::ptr::null(), &mut ds_layout) != VK_SUCCESS {
                    return None;
                }

                let pool_size = VkDescriptorPoolSize { ty: VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, descriptor_count: 256 };
                let dp_ci = VkDescriptorPoolCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT,
                    max_sets: 256,
                    pool_size_count: 1,
                    p_pool_sizes: &pool_size,
                };
                let mut ds_pool = 0;
                if (fns.create_descriptor_pool)(device, &dp_ci, std::ptr::null(), &mut ds_pool) != VK_SUCCESS {
                    return None;
                }

                let mk_sampler = |filter: i32| -> Option<VkSampler> {
                    let ci = VkSamplerCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                        mag_filter: filter,
                        min_filter: filter,
                        mipmap_mode: 0,
                        address_mode_u: 2, // CLAMP_TO_EDGE
                        address_mode_v: 2,
                        address_mode_w: 2,
                        mip_lod_bias: 0.0,
                        anisotropy_enable: VK_FALSE,
                        max_anisotropy: 1.0,
                        compare_enable: VK_FALSE,
                        compare_op: 0,
                        min_lod: 0.0,
                        max_lod: 0.0,
                        border_color: 0,
                        unnormalized_coordinates: VK_FALSE,
                    };
                    let mut s = 0;
                    ((fns.create_sampler)(device, &ci, std::ptr::null(), &mut s) == VK_SUCCESS).then_some(s)
                };
                let sampler_lin = mk_sampler(1)?; // VK_FILTER_LINEAR
                let sampler_near = mk_sampler(0)?; // VK_FILTER_NEAREST

                // Pipeline layout with 16-byte push constants.
                let pc = VkPushConstantRange {
                    stage_flags: VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT,
                    offset: 0,
                    size: 16,
                };
                let pl_ci = VkPipelineLayoutCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    set_layout_count: 1,
                    p_set_layouts: &ds_layout,
                    push_constant_range_count: 1,
                    p_push_constant_ranges: &pc,
                };
                let mut pipe_layout = 0;
                if (fns.create_pipeline_layout)(device, &pl_ci, std::ptr::null(), &mut pipe_layout) != VK_SUCCESS {
                    return None;
                }

                // Shader modules & graphics pipeline.
                let mk_mod = |words: &[u32]| -> Option<VkShaderModule> {
                    let ci = VkShaderModuleCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                        code_size: words.len() * 4,
                        p_code: words.as_ptr(),
                    };
                    let mut m = 0;
                    ((fns.create_shader_module)(device, &ci, std::ptr::null(), &mut m) == VK_SUCCESS).then_some(m)
                };
                let vs_mod = mk_mod(VS_SPV)?;
                let fs_mod = mk_mod(FS_SPV)?;
                let stages = [
                    VkPipelineShaderStageCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                        stage: VK_SHADER_STAGE_VERTEX_BIT,
                        module: vs_mod,
                        p_name: c"main".as_ptr(),
                        p_specialization_info: std::ptr::null(),
                    },
                    VkPipelineShaderStageCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                        stage: VK_SHADER_STAGE_FRAGMENT_BIT,
                        module: fs_mod,
                        p_name: c"main".as_ptr(),
                        p_specialization_info: std::ptr::null(),
                    },
                ];
                let vbind = VkVertexInputBindingDescription { binding: 0, stride: VERTEX_SIZE as u32, input_rate: 0 };
                let vattrs = [
                    VkVertexInputAttributeDescription { location: 0, binding: 0, format: VK_FORMAT_R32G32_SFLOAT, offset: 0 },
                    VkVertexInputAttributeDescription { location: 1, binding: 0, format: VK_FORMAT_R32G32_SFLOAT, offset: 8 },
                    VkVertexInputAttributeDescription { location: 2, binding: 0, format: VK_FORMAT_R8G8B8A8_UNORM, offset: 16 },
                ];
                let vi = VkPipelineVertexInputStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    vertex_binding_description_count: 1,
                    p_vertex_binding_descriptions: &vbind,
                    vertex_attribute_description_count: vattrs.len() as u32,
                    p_vertex_attribute_descriptions: vattrs.as_ptr(),
                };
                let ia = VkPipelineInputAssemblyStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    topology: 3, // TRIANGLE_LIST
                    primitive_restart_enable: VK_FALSE,
                };
                let vp_state = VkPipelineViewportStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    viewport_count: 1,
                    p_viewports: std::ptr::null(),
                    scissor_count: 1,
                    p_scissors: std::ptr::null(),
                };
                let rast = VkPipelineRasterizationStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    depth_clamp_enable: VK_FALSE,
                    rasterizer_discard_enable: VK_FALSE,
                    polygon_mode: 0,
                    cull_mode: 0,
                    front_face: 0,
                    depth_bias_enable: VK_FALSE,
                    depth_bias_constant_factor: 0.0,
                    depth_bias_clamp: 0.0,
                    depth_bias_slope_factor: 0.0,
                    line_width: 1.0,
                };
                let ms = VkPipelineMultisampleStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    rasterization_samples: 1,
                    sample_shading_enable: VK_FALSE,
                    min_sample_shading: 0.0,
                    p_sample_mask: std::ptr::null(),
                    alpha_to_coverage_enable: VK_FALSE,
                    alpha_to_one_enable: VK_FALSE,
                };
                let blend_att = VkPipelineColorBlendAttachmentState {
                    blend_enable: VK_TRUE,
                    src_color_blend_factor: 1, // ONE
                    dst_color_blend_factor: 7, // ONE_MINUS_SRC_ALPHA
                    color_blend_op: 0,         // ADD
                    src_alpha_blend_factor: 9, // ONE_MINUS_DST_ALPHA
                    dst_alpha_blend_factor: 1, // ONE
                    alpha_blend_op: 0,
                    color_write_mask: 0xF,
                };
                let blend = VkPipelineColorBlendStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    logic_op_enable: VK_FALSE,
                    logic_op: 0,
                    attachment_count: 1,
                    p_attachments: &blend_att,
                    blend_constants: [0.0; 4],
                };
                let dyn_states = [0i32, 1i32]; // VIEWPORT, SCISSOR
                let dyn_ci = VkPipelineDynamicStateCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    dynamic_state_count: 2,
                    p_dynamic_states: dyn_states.as_ptr(),
                };
                let gp_ci = VkGraphicsPipelineCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    stage_count: 2,
                    p_stages: stages.as_ptr(),
                    p_vertex_input_state: &vi,
                    p_input_assembly_state: &ia,
                    p_tessellation_state: std::ptr::null(),
                    p_viewport_state: &vp_state,
                    p_rasterization_state: &rast,
                    p_multisample_state: &ms,
                    p_depth_stencil_state: std::ptr::null(),
                    p_color_blend_state: &blend,
                    p_dynamic_state: &dyn_ci,
                    layout: pipe_layout,
                    render_pass,
                    subpass: 0,
                    base_pipeline_handle: 0,
                    base_pipeline_index: -1,
                };
                let mut pipeline = 0;
                let pipe_res = (fns.create_graphics_pipelines)(device, 0, 1, &gp_ci, std::ptr::null(), &mut pipeline);
                (fns.destroy_shader_module)(device, vs_mod, std::ptr::null());
                (fns.destroy_shader_module)(device, fs_mod, std::ptr::null());
                if pipe_res != VK_SUCCESS {
                    return None;
                }

                // Command pool & per-image slots.
                let cp_ci = VkCommandPoolCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
                    queue_family_index: queue_family,
                };
                let mut cmd_pool = 0;
                if (fns.create_command_pool)(device, &cp_ci, std::ptr::null(), &mut cmd_pool) != VK_SUCCESS {
                    return None;
                }

                let mut slots = Vec::with_capacity(images.len());
                for img in images {
                    let iv_ci = VkImageViewCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                        image: img,
                        view_type: 1, // 2D
                        format,
                        components: VkComponentMapping { r: 0, g: 0, b: 0, a: 0 },
                        subresource_range: VkImageSubresourceRange {
                            aspect_mask: VK_IMAGE_ASPECT_COLOR_BIT,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        },
                    };
                    let mut view = 0;
                    if (fns.create_image_view)(device, &iv_ci, std::ptr::null(), &mut view) != VK_SUCCESS {
                        return None;
                    }
                    let fb_ci = VkFramebufferCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                        render_pass,
                        attachment_count: 1,
                        p_attachments: &view,
                        width: extent.width,
                        height: extent.height,
                        layers: 1,
                    };
                    let mut fb = 0;
                    if (fns.create_framebuffer)(device, &fb_ci, std::ptr::null(), &mut fb) != VK_SUCCESS {
                        return None;
                    }
                    let cb_ai = VkCommandBufferAllocateInfo {
                        s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                        p_next: std::ptr::null(),
                        command_pool: cmd_pool,
                        level: 0,
                        command_buffer_count: 1,
                    };
                    let mut cmd = std::ptr::null_mut();
                    if (fns.allocate_command_buffers)(device, &cb_ai, &mut cmd) != VK_SUCCESS {
                        return None;
                    }
                    let f_ci = VkFenceCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_FENCE_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: VK_FENCE_CREATE_SIGNALED_BIT,
                    };
                    let mut fence = 0;
                    if (fns.create_fence)(device, &f_ci, std::ptr::null(), &mut fence) != VK_SUCCESS {
                        return None;
                    }
                    let s_ci = VkSemaphoreCreateInfo {
                        s_type: VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,
                        p_next: std::ptr::null(),
                        flags: 0,
                    };
                    let mut done_sem = 0;
                    if (fns.create_semaphore)(device, &s_ci, std::ptr::null(), &mut done_sem) != VK_SUCCESS {
                        return None;
                    }
                    slots.push(VkFrameSlot {
                        view,
                        fb,
                        cmd,
                        fence,
                        done_sem,
                        vb: None,
                        ib: None,
                        staging: Vec::new(),
                        last_submit_gen: 0,
                        fence_pending: false,
                    });
                }

                Some(Self {
                    device,
                    fns,
                    mem_props,
                    hwnd,
                    size: (extent.width, extent.height),
                    linear_out: util::vk_format_is_linear_out(format),
                    render_pass,
                    ds_layout,
                    ds_pool,
                    sampler_lin,
                    sampler_near,
                    pipe_layout,
                    pipeline,
                    cmd_pool,
                    slots,
                    textures: HashMap::new(),
                    submit_gen: 0,
                    retired_tex: Vec::new(),
                })
            }
        }

        unsafe fn create_host_buffer(&self, bytes: usize, usage: VkFlags) -> Option<VkBuf> {
            unsafe {
                let ci = VkBufferCreateInfo {
                    s_type: VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
                    p_next: std::ptr::null(),
                    flags: 0,
                    size: bytes as VkDeviceSize,
                    usage,
                    sharing_mode: 0,
                    queue_family_index_count: 0,
                    p_queue_family_indices: std::ptr::null(),
                };
                let mut buf = 0;
                if (self.fns.create_buffer)(self.device, &ci, std::ptr::null(), &mut buf) != VK_SUCCESS {
                    return None;
                }
                let mut req = VkMemoryRequirements::default();
                (self.fns.get_buffer_memory_requirements)(self.device, buf, &mut req);
                let Some(mem_type) = self.find_mem_type(req.memory_type_bits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) else {
                    (self.fns.destroy_buffer)(self.device, buf, std::ptr::null());
                    return None;
                };
                let ai = VkMemoryAllocateInfo {
                    s_type: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                    p_next: std::ptr::null(),
                    allocation_size: req.size,
                    memory_type_index: mem_type,
                };
                let mut mem = 0;
                if (self.fns.allocate_memory)(self.device, &ai, std::ptr::null(), &mut mem) != VK_SUCCESS {
                    (self.fns.destroy_buffer)(self.device, buf, std::ptr::null());
                    return None;
                }
                if (self.fns.bind_buffer_memory)(self.device, buf, mem, 0) != VK_SUCCESS {
                    (self.fns.free_memory)(self.device, mem, std::ptr::null());
                    (self.fns.destroy_buffer)(self.device, buf, std::ptr::null());
                    return None;
                }
                let mut mapped = std::ptr::null_mut();
                if (self.fns.map_memory)(self.device, mem, 0, bytes as VkDeviceSize, 0, &mut mapped) != VK_SUCCESS {
                    (self.fns.free_memory)(self.device, mem, std::ptr::null());
                    (self.fns.destroy_buffer)(self.device, buf, std::ptr::null());
                    return None;
                }
                Some(VkBuf { buf, mem, mapped: mapped as *mut u8, cap: bytes })
            }
        }

        unsafe fn destroy_tex(&self, t: VkTex) {
            unsafe {
                let _ = (self.fns.free_descriptor_sets)(self.device, self.ds_pool, 1, &t.set);
                (self.fns.destroy_image_view)(self.device, t.view, std::ptr::null());
                (self.fns.destroy_image)(self.device, t.image, std::ptr::null());
                (self.fns.free_memory)(self.device, t.mem, std::ptr::null());
            }
        }

        /// Reclaim retired textures once every pending swapchain slot that could
        /// reference them has finished executing on the GPU.
        unsafe fn reclaim_retired_textures(&mut self) {
            let min_pending = self
                .slots
                .iter()
                .filter(|s| s.fence_pending)
                .map(|s| s.last_submit_gen)
                .min_by(|a, b| (a.wrapping_sub(*b) as i64).cmp(&0));
            let mut i = 0;
            while i < self.retired_tex.len() {
                let safe = match min_pending {
                    None => true,
                    Some(min_gen) => (min_gen.wrapping_sub(self.retired_tex[i].0) as i64) > 0,
                };
                if safe {
                    let (_, t) = self.retired_tex.swap_remove(i);
                    unsafe { self.destroy_tex(t) };
                } else {
                    i += 1;
                }
            }
        }

        unsafe fn apply_tex_op(&mut self, cmd: VkCommandBuffer, op: TexOp, staging: &mut Vec<(VkBuffer, VkDeviceMemory)>) {
            // Tag retirements with the generation of the batch currently being
            // recorded (submit_gen + 1) so an upload and free/replace in the
            // same batch cannot be reclaimed while this batch is still pending.
            let retire_gen = self.submit_gen.wrapping_add(1);
            match op {
                TexOp::Free(id) => {
                    if let Some(t) = self.textures.remove(&id) {
                        self.retired_tex.push((retire_gen, t));
                    }
                }
                TexOp::Set(t) => unsafe {
                    let [w, h] = t.size;
                    let Some((_, needed)) = util::validate_tex_rgba(t.size, t.rgba.len()) else {
                        return;
                    };
                    if let Some([px, py]) = t.pos {
                        let Some(tex) = self.textures.get(&t.id) else { return };
                        if !util::validate_tex_subrect([px, py], t.size, tex.size) {
                            return;
                        }
                    }
                    let Some(sb) = self.create_host_buffer(needed, VK_BUFFER_USAGE_TRANSFER_SRC_BIT) else { return };
                    std::ptr::copy_nonoverlapping(t.rgba.as_ptr(), sb.mapped, needed);
                    (self.fns.unmap_memory)(self.device, sb.mem);

                    let free_sb = |fns: &VkDevFns, dev: VkDevice, sb: &VkBuf| {
                        (fns.destroy_buffer)(dev, sb.buf, std::ptr::null());
                        (fns.free_memory)(dev, sb.mem, std::ptr::null());
                    };

                    let (dst_img, dx, dy, old_layout) = match t.pos {
                        Some([px, py]) => {
                            let Some(tex) = self.textures.get_mut(&t.id) else {
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            };
                            tex.linear = t.linear;
                            (tex.image, px as i32, py as i32, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL)
                        }
                        None => {
                            let ic = VkImageCreateInfo {
                                s_type: VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                                p_next: std::ptr::null(),
                                flags: 0,
                                image_type: 1, // 2D
                                format: VK_FORMAT_R8G8B8A8_UNORM,
                                extent: VkExtent3D { width: w, height: h, depth: 1 },
                                mip_levels: 1,
                                array_layers: 1,
                                samples: 1,
                                tiling: 0, // OPTIMAL
                                usage: VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT,
                                sharing_mode: 0,
                                queue_family_index_count: 0,
                                p_queue_family_indices: std::ptr::null(),
                                initial_layout: VK_IMAGE_LAYOUT_UNDEFINED,
                            };
                            let mut image = 0;
                            if (self.fns.create_image)(self.device, &ic, std::ptr::null(), &mut image) != VK_SUCCESS {
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            }
                            let mut req = VkMemoryRequirements::default();
                            (self.fns.get_image_memory_requirements)(self.device, image, &mut req);
                            let Some(mem_type) = self.find_mem_type(req.memory_type_bits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) else {
                                (self.fns.destroy_image)(self.device, image, std::ptr::null());
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            };
                            let ai = VkMemoryAllocateInfo {
                                s_type: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                                p_next: std::ptr::null(),
                                allocation_size: req.size,
                                memory_type_index: mem_type,
                            };
                            let mut mem = 0;
                            if (self.fns.allocate_memory)(self.device, &ai, std::ptr::null(), &mut mem) != VK_SUCCESS {
                                (self.fns.destroy_image)(self.device, image, std::ptr::null());
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            }
                            if (self.fns.bind_image_memory)(self.device, image, mem, 0) != VK_SUCCESS {
                                (self.fns.free_memory)(self.device, mem, std::ptr::null());
                                (self.fns.destroy_image)(self.device, image, std::ptr::null());
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            }
                            let iv_ci = VkImageViewCreateInfo {
                                s_type: VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
                                p_next: std::ptr::null(),
                                flags: 0,
                                image,
                                view_type: 1,
                                format: VK_FORMAT_R8G8B8A8_UNORM,
                                components: VkComponentMapping { r: 0, g: 0, b: 0, a: 0 },
                                subresource_range: VkImageSubresourceRange {
                                    aspect_mask: VK_IMAGE_ASPECT_COLOR_BIT,
                                    base_mip_level: 0,
                                    level_count: 1,
                                    base_array_layer: 0,
                                    layer_count: 1,
                                },
                            };
                            let mut view = 0;
                            if (self.fns.create_image_view)(self.device, &iv_ci, std::ptr::null(), &mut view) != VK_SUCCESS {
                                (self.fns.free_memory)(self.device, mem, std::ptr::null());
                                (self.fns.destroy_image)(self.device, image, std::ptr::null());
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            }
                            let ds_ai = VkDescriptorSetAllocateInfo {
                                s_type: VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO,
                                p_next: std::ptr::null(),
                                descriptor_pool: self.ds_pool,
                                descriptor_set_count: 1,
                                p_set_layouts: &self.ds_layout,
                            };
                            let mut set = 0;
                            if (self.fns.allocate_descriptor_sets)(self.device, &ds_ai, &mut set) != VK_SUCCESS {
                                (self.fns.destroy_image_view)(self.device, view, std::ptr::null());
                                (self.fns.free_memory)(self.device, mem, std::ptr::null());
                                (self.fns.destroy_image)(self.device, image, std::ptr::null());
                                free_sb(&self.fns, self.device, &sb);
                                return;
                            }
                            let di = VkDescriptorImageInfo {
                                sampler: if t.linear { self.sampler_lin } else { self.sampler_near },
                                image_view: view,
                                image_layout: VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
                            };
                            let wds = VkWriteDescriptorSet {
                                s_type: VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET,
                                p_next: std::ptr::null(),
                                dst_set: set,
                                dst_binding: 0,
                                dst_array_element: 0,
                                descriptor_count: 1,
                                descriptor_type: VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
                                p_image_info: &di,
                                p_buffer_info: std::ptr::null(),
                                p_texel_buffer_view: std::ptr::null(),
                            };
                            (self.fns.update_descriptor_sets)(self.device, 1, &wds, 0, std::ptr::null());
                            if let Some(old) = self.textures.insert(t.id, VkTex { image, mem, view, set, size: t.size, linear: t.linear }) {
                                self.retired_tex.push((retire_gen, old));
                            }
                            (image, 0, 0, VK_IMAGE_LAYOUT_UNDEFINED)
                        }
                    };

                    let range = VkImageSubresourceRange {
                        aspect_mask: VK_IMAGE_ASPECT_COLOR_BIT,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    };
                    let b1 = VkImageMemoryBarrier {
                        s_type: VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                        p_next: std::ptr::null(),
                        src_access_mask: if old_layout == VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL { VK_ACCESS_SHADER_READ_BIT } else { 0 },
                        dst_access_mask: VK_ACCESS_TRANSFER_WRITE_BIT,
                        old_layout,
                        new_layout: VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                        src_queue_family_index: u32::MAX,
                        dst_queue_family_index: u32::MAX,
                        image: dst_img,
                        subresource_range: range,
                    };
                    (self.fns.cmd_pipeline_barrier)(
                        cmd,
                        if old_layout == VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL { VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT } else { VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT },
                        VK_PIPELINE_STAGE_TRANSFER_BIT,
                        0, 0, std::ptr::null(), 0, std::ptr::null(), 1, &b1,
                    );
                    let copy = VkBufferImageCopy {
                        buffer_offset: 0,
                        buffer_row_length: 0,
                        buffer_image_height: 0,
                        image_subresource: VkImageSubresourceLayers { aspect_mask: VK_IMAGE_ASPECT_COLOR_BIT, mip_level: 0, base_array_layer: 0, layer_count: 1 },
                        image_offset: VkOffset3D { x: dx, y: dy, z: 0 },
                        image_extent: VkExtent3D { width: w, height: h, depth: 1 },
                    };
                    (self.fns.cmd_copy_buffer_to_image)(cmd, sb.buf, dst_img, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &copy);
                    let b2 = VkImageMemoryBarrier {
                        s_type: VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                        p_next: std::ptr::null(),
                        src_access_mask: VK_ACCESS_TRANSFER_WRITE_BIT,
                        dst_access_mask: VK_ACCESS_SHADER_READ_BIT,
                        old_layout: VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                        new_layout: VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
                        src_queue_family_index: u32::MAX,
                        dst_queue_family_index: u32::MAX,
                        image: dst_img,
                        subresource_range: range,
                    };
                    (self.fns.cmd_pipeline_barrier)(
                        cmd,
                        VK_PIPELINE_STAGE_TRANSFER_BIT,
                        VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                        0, 0, std::ptr::null(), 0, std::ptr::null(), 1, &b2,
                    );
                    staging.push((sb.buf, sb.mem));
                },
            }
        }

        unsafe fn render(
            &mut self,
            queue: VkQueue,
            img_idx: usize,
            wait_sems: &[VkSemaphore],
            ops: Vec<TexOp>,
            frame: Option<&Frame>,
        ) -> Option<VkSemaphore> {
            if img_idx >= self.slots.len() {
                return None;
            }
            unsafe {
                let fence = self.slots[img_idx].fence;
                let cmd = self.slots[img_idx].cmd;
                let fb = self.slots[img_idx].fb;
                let done_sem = self.slots[img_idx].done_sem;

                if self.slots[img_idx].fence_pending {
                    if (self.fns.wait_for_fences)(self.device, 1, &fence, VK_TRUE, u64::MAX) != VK_SUCCESS {
                        return None;
                    }
                    self.slots[img_idx].fence_pending = false;
                }
                self.reclaim_retired_textures();
                for (b, m) in self.slots[img_idx].staging.drain(..) {
                    (self.fns.destroy_buffer)(self.device, b, std::ptr::null());
                    (self.fns.free_memory)(self.device, m, std::ptr::null());
                }
                (self.fns.reset_command_buffer)(cmd, 0);
                let bi = VkCommandBufferBeginInfo {
                    s_type: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                    p_next: std::ptr::null(),
                    flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
                    p_inheritance_info: std::ptr::null(),
                };
                if (self.fns.begin_command_buffer)(cmd, &bi) != VK_SUCCESS {
                    return None;
                }

                let mut staging = std::mem::take(&mut self.slots[img_idx].staging);
                for op in ops {
                    self.apply_tex_op(cmd, op, &mut staging);
                }

                if let Some(frame) = frame.filter(|f| !f.meshes.is_empty())
                    && let Some((vbytes, ibytes)) = util::validate_mesh_totals(&frame.meshes, VERTEX_SIZE)
                    && vbytes > 0
                    && ibytes > 0
                {
                    let mut vb_opt = self.slots[img_idx].vb.take();
                    let mut ib_opt = self.slots[img_idx].ib.take();
                    let ensure_buf = |slot: &mut Option<VkBuf>, need: usize, usage: VkFlags| -> Option<(VkBuffer, *mut u8)> {
                        if let Some(b) = slot.as_ref().filter(|b| b.cap >= need) {
                            return Some((b.buf, b.mapped));
                        }
                        let cap = util::checked_grow_capacity(need, 64 * 1024)?;
                        if let Some(old) = slot.take() {
                            (self.fns.unmap_memory)(self.device, old.mem);
                            (self.fns.destroy_buffer)(self.device, old.buf, std::ptr::null());
                            (self.fns.free_memory)(self.device, old.mem, std::ptr::null());
                        }
                        let b = self.create_host_buffer(cap, usage)?;
                        let res = (b.buf, b.mapped);
                        *slot = Some(b);
                        Some(res)
                    };
                    let vb_pair = ensure_buf(&mut vb_opt, vbytes, VK_BUFFER_USAGE_VERTEX_BUFFER_BIT);
                    let ib_pair = ensure_buf(&mut ib_opt, ibytes, VK_BUFFER_USAGE_INDEX_BUFFER_BIT);
                    self.slots[img_idx].vb = vb_opt;
                    self.slots[img_idx].ib = ib_opt;

                    if let (Some((vb, vptr)), Some((ib, iptr))) = (vb_pair, ib_pair) {
                        let (mut vat, mut iat) = (vptr, iptr);
                        for m in &frame.meshes {
                            std::ptr::copy_nonoverlapping(m.vertices.as_ptr(), vat, m.vertices.len());
                            vat = vat.add(m.vertices.len());
                            let ilen = m.indices.len() * 4;
                            std::ptr::copy_nonoverlapping(m.indices.as_ptr() as *const u8, iat, ilen);
                            iat = iat.add(ilen);
                        }

                        let rp_bi = VkRenderPassBeginInfo {
                            s_type: VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
                            p_next: std::ptr::null(),
                            render_pass: self.render_pass,
                            framebuffer: fb,
                            render_area: VkRect2D {
                                offset: VkOffset2D { x: 0, y: 0 },
                                extent: VkExtent2D { width: self.size.0, height: self.size.1 },
                            },
                            clear_value_count: 0,
                            p_clear_values: std::ptr::null(),
                        };
                        (self.fns.cmd_begin_render_pass)(cmd, &rp_bi, 0);
                        (self.fns.cmd_bind_pipeline)(cmd, 0, self.pipeline);
                        let vp = VkViewport { x: 0.0, y: 0.0, width: self.size.0 as f32, height: self.size.1 as f32, min_depth: 0.0, max_depth: 1.0 };
                        (self.fns.cmd_set_viewport)(cmd, 0, 1, &vp);
                        let zero_off: VkDeviceSize = 0;
                        (self.fns.cmd_bind_vertex_buffers)(cmd, 0, 1, &vb, &zero_off);
                        (self.fns.cmd_bind_index_buffer)(cmd, ib, 0, VK_INDEX_TYPE_UINT32);

                        let ppp = frame.pixels_per_point.max(0.1);
                        let push = [self.size.0 as f32 / ppp, self.size.1 as f32 / ppp, if self.linear_out { 1.0 } else { 0.0 }, 0.0f32];
                        (self.fns.cmd_push_constants)(
                            cmd,
                            self.pipe_layout,
                            VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT,
                            0,
                            16,
                            push.as_ptr().cast(),
                        );

                        let (mut first_index, mut base_vertex) = (0u32, 0i32);
                        for m in &frame.meshes {
                            let count = m.indices.len() as u32;
                            let verts = (m.vertices.len() / VERTEX_SIZE) as i32;
                            if let Some([left, top, right, bottom]) = util::clip_rect_pixels(m.clip, ppp, self.size.0, self.size.1) {
                                if let Some(tex) = self.textures.get(&m.texture).filter(|_| count > 0) {
                                    let sc = VkRect2D {
                                        offset: VkOffset2D { x: left, y: top },
                                        extent: VkExtent2D { width: (right - left) as u32, height: (bottom - top) as u32 },
                                    };
                                    (self.fns.cmd_set_scissor)(cmd, 0, 1, &sc);
                                    (self.fns.cmd_bind_descriptor_sets)(cmd, 0, self.pipe_layout, 0, 1, &tex.set, 0, std::ptr::null());
                                    (self.fns.cmd_draw_indexed)(cmd, count, 1, first_index, base_vertex, 0);
                                }
                            }
                            first_index += count;
                            base_vertex += verts;
                        }
                        (self.fns.cmd_end_render_pass)(cmd);
                    }
                }

                self.slots[img_idx].staging = staging;
                if (self.fns.end_command_buffer)(cmd) != VK_SUCCESS {
                    return None;
                }

                let stages = [VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT; 8];
                let overflow;
                let wait_stages = if let Some(slice) = stages.get(..wait_sems.len()) {
                    slice
                } else {
                    overflow = vec![VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT; wait_sems.len()];
                    &overflow
                };
                let si = VkSubmitInfo {
                    s_type: VK_STRUCTURE_TYPE_SUBMIT_INFO,
                    p_next: std::ptr::null(),
                    wait_semaphore_count: wait_sems.len() as u32,
                    p_wait_semaphores: if wait_sems.is_empty() { std::ptr::null() } else { wait_sems.as_ptr() },
                    p_wait_dst_stage_mask: if wait_stages.is_empty() { std::ptr::null() } else { wait_stages.as_ptr() },
                    command_buffer_count: 1,
                    p_command_buffers: &cmd,
                    signal_semaphore_count: 1,
                    p_signal_semaphores: &done_sem,
                };
                if (self.fns.reset_fences)(self.device, 1, &fence) != VK_SUCCESS {
                    return None;
                }
                if (self.fns.queue_submit)(queue, 1, &si, fence) != VK_SUCCESS {
                    return None;
                }
                self.submit_gen = self.submit_gen.wrapping_add(1);
                self.slots[img_idx].last_submit_gen = self.submit_gen;
                self.slots[img_idx].fence_pending = true;
                Some(done_sem)
            }
        }

        unsafe fn destroy(&mut self) {
            unsafe {
                let _ = (self.fns.device_wait_idle)(self.device);
                let textures = std::mem::take(&mut self.textures);
                for (_, t) in textures {
                    self.destroy_tex(t);
                }
                let retired = std::mem::take(&mut self.retired_tex);
                for (_, t) in retired {
                    self.destroy_tex(t);
                }
                for s in self.slots.drain(..) {
                    for (b, m) in s.staging {
                        (self.fns.destroy_buffer)(self.device, b, std::ptr::null());
                        (self.fns.free_memory)(self.device, m, std::ptr::null());
                    }
                    if let Some(vb) = s.vb {
                        (self.fns.unmap_memory)(self.device, vb.mem);
                        (self.fns.destroy_buffer)(self.device, vb.buf, std::ptr::null());
                        (self.fns.free_memory)(self.device, vb.mem, std::ptr::null());
                    }
                    if let Some(ib) = s.ib {
                        (self.fns.unmap_memory)(self.device, ib.mem);
                        (self.fns.destroy_buffer)(self.device, ib.buf, std::ptr::null());
                        (self.fns.free_memory)(self.device, ib.mem, std::ptr::null());
                    }
                    (self.fns.destroy_semaphore)(self.device, s.done_sem, std::ptr::null());
                    (self.fns.destroy_fence)(self.device, s.fence, std::ptr::null());
                    (self.fns.destroy_framebuffer)(self.device, s.fb, std::ptr::null());
                    (self.fns.destroy_image_view)(self.device, s.view, std::ptr::null());
                }
                (self.fns.destroy_command_pool)(self.device, self.cmd_pool, std::ptr::null());
                (self.fns.destroy_pipeline)(self.device, self.pipeline, std::ptr::null());
                (self.fns.destroy_pipeline_layout)(self.device, self.pipe_layout, std::ptr::null());
                (self.fns.destroy_sampler)(self.device, self.sampler_lin, std::ptr::null());
                (self.fns.destroy_sampler)(self.device, self.sampler_near, std::ptr::null());
                (self.fns.destroy_descriptor_pool)(self.device, self.ds_pool, std::ptr::null());
                (self.fns.destroy_descriptor_set_layout)(self.device, self.ds_layout, std::ptr::null());
                (self.fns.destroy_render_pass)(self.device, self.render_pass, std::ptr::null());
            }
        }
    }
}

#[cfg(windows)]
pub use win_impl::install_if_loaded;

#[cfg(test)]
mod tests {
    use super::spv::{FS_SPV, VS_SPV};

    #[test]
    fn embedded_vulkan_spirv_is_valid() {
        assert!(super::super::util::validate_spirv(VS_SPV));
        assert!(super::super::util::validate_spirv(FS_SPV));
    }
}
