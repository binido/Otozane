// Otozane Microphone: a Core Audio server plug-in with one loopback device.
// otozaned plays the processed microphone into its output stream, and apps
// record the same audio from its input stream. Output from any other process
// is dropped, so nothing else can leak into the microphone.

#include <aspl/Driver.hpp>

#include <CoreAudio/AudioServerPlugIn.h>

#include <atomic>
#include <cmath>
#include <cstring>

namespace {

constexpr UInt32 SampleRate = 48000;
constexpr UInt32 Channels = 2;
// Power of two, about 0.7 s at 48 kHz.
constexpr SInt64 RingFrames = 32768;
// Clients that may write. otozaned carries its bundle id in an embedded Info.plist.
constexpr const char* WriterBundleID = "io.github.binido.otozane.service";

AudioStreamBasicDescription Float32Format()
{
    return {
        .mSampleRate = SampleRate,
        .mFormatID = kAudioFormatLinearPCM,
        .mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian |
                        kAudioFormatFlagIsPacked,
        .mBytesPerPacket = Channels * sizeof(Float32),
        .mFramesPerPacket = 1,
        .mBytesPerFrame = Channels * sizeof(Float32),
        .mChannelsPerFrame = Channels,
        .mBitsPerChannel = 32,
    };
}

// The ring is indexed by device sample time. The writer fills frames ahead of
// the present, readers read frames just behind it at the same positions.
class Loopback : public aspl::ControlRequestHandler, public aspl::IORequestHandler
{
public:
    void OnWriteClientOutput(const std::shared_ptr<aspl::Client>& client,
        const std::shared_ptr<aspl::Stream>&,
        Float64,
        Float64 timestamp,
        const Float32* frames,
        UInt32 frameCount,
        UInt32 channelCount) override
    {
        if (client->GetBundleID() != WriterBundleID || channelCount != Channels) {
            return;
        }
        const SInt64 start = std::llround(timestamp);
        // A gap in the writes starts a new run. Frames before it are stale.
        if (start != writeEnd_.load(std::memory_order_relaxed)) {
            runStart_.store(start, std::memory_order_relaxed);
        }
        for (UInt32 i = 0; i < frameCount; i++) {
            Float32* dst = &ring_[((start + i) & (RingFrames - 1)) * Channels];
            std::memcpy(dst, &frames[i * Channels], Channels * sizeof(Float32));
        }
        writeEnd_.store(start + frameCount, std::memory_order_release);
    }

    void OnReadClientInput(const std::shared_ptr<aspl::Client>&,
        const std::shared_ptr<aspl::Stream>&,
        Float64,
        Float64 timestamp,
        void* bytes,
        UInt32 bytesCount) override
    {
        auto* out = static_cast<Float32*>(bytes);
        const UInt32 frameCount = bytesCount / (Channels * sizeof(Float32));
        const SInt64 start = std::llround(timestamp);
        const SInt64 end = writeEnd_.load(std::memory_order_acquire);
        const SInt64 run = runStart_.load(std::memory_order_relaxed);
        for (UInt32 i = 0; i < frameCount; i++) {
            const SInt64 t = start + i;
            // Only frames of the current run that the ring still holds.
            if (t >= run && t < end && end - t <= RingFrames) {
                std::memcpy(&out[i * Channels],
                    &ring_[(t & (RingFrames - 1)) * Channels],
                    Channels * sizeof(Float32));
            } else {
                std::memset(&out[i * Channels], 0, Channels * sizeof(Float32));
            }
        }
    }

private:
    Float32 ring_[RingFrames * Channels] = {};
    std::atomic<SInt64> writeEnd_{-1};
    std::atomic<SInt64> runStart_{0};
};

// Offered as a default input, never as a default output: only otozaned may
// write to it, so system sound routed here would just be dropped.
class Microphone : public aspl::Device
{
public:
    using aspl::Device::Device;

    OSStatus GetPropertyData(AudioObjectID objectID,
        pid_t clientPID,
        const AudioObjectPropertyAddress* address,
        UInt32 qualifierDataSize,
        const void* qualifierData,
        UInt32 inDataSize,
        UInt32* outDataSize,
        void* outData) const override
    {
        const bool canBeDefault =
            address->mSelector == kAudioDevicePropertyDeviceCanBeDefaultDevice ||
            address->mSelector == kAudioDevicePropertyDeviceCanBeDefaultSystemDevice;
        if (canBeDefault && inDataSize >= sizeof(UInt32)) {
            *static_cast<UInt32*>(outData) =
                address->mScope == kAudioObjectPropertyScopeInput ? 1 : 0;
            *outDataSize = sizeof(UInt32);
            return kAudioHardwareNoError;
        }
        return aspl::Device::GetPropertyData(objectID, clientPID, address,
            qualifierDataSize, qualifierData, inDataSize, outDataSize, outData);
    }
};

std::shared_ptr<aspl::Driver> CreateDriver()
{
    auto context = std::make_shared<aspl::Context>();

    aspl::DeviceParameters params;
    params.Name = "Otozane Microphone";
    params.Manufacturer = "Otozane";
    params.DeviceUID = "OtozaneMicrophone_UID";
    params.ModelUID = "OtozaneMicrophone_ModelUID";
    params.SampleRate = SampleRate;
    params.ChannelCount = Channels;
    // Per-client writes, so the writer can be checked.
    params.EnableMixing = false;

    auto device = std::make_shared<Microphone>(context, params);
    for (auto dir : {aspl::Direction::Output, aspl::Direction::Input}) {
        aspl::StreamParameters stream;
        stream.Direction = dir;
        stream.Format = Float32Format();
        device->AddStreamAsync(stream);
    }
    auto loopback = std::make_shared<Loopback>();
    device->SetControlHandler(loopback);
    device->SetIOHandler(loopback);

    auto plugin = std::make_shared<aspl::Plugin>(context);
    plugin->AddDevice(device);
    return std::make_shared<aspl::Driver>(context, plugin);
}

} // namespace

extern "C" void* OtozaneDriverEntryPoint(CFAllocatorRef, CFUUIDRef typeUUID)
{
    if (!CFEqual(typeUUID, kAudioServerPlugInTypeUUID)) {
        return nullptr;
    }
    static std::shared_ptr<aspl::Driver> driver = CreateDriver();
    return driver->GetReference();
}
