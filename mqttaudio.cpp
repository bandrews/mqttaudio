#include <argp.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <sysexits.h>
#include <unistd.h>

#include <vector>
#include <iostream>
#include <string>
#include <unordered_map>

#include <mosquitto.h>

#include "rapidjson/document.h"
#include "rapidjson/writer.h"
#include "rapidjson/stringbuffer.h"

#include "SDL.h"
#include "SDL_mixer.h"

#include "alsautil.h"
#include "sample.h"
#include "samplemanager.h"
#include "SDL_rwhttp.h"

using namespace std;
using namespace rapidjson;

// Structure to track playback information for each channel
struct ChannelPlaybackInfo {
    std::string file;  // Original file path used to start playback
    Sample* sample;
    float volume;
    bool loop;
    int maxPlayLength;
    Mix_Chunk* seekChunk;  // Non-null if playing from a seek position
};

// Map of channel number to playback info
std::unordered_map<int, ChannelPlaybackInfo> channelInfo;

const char *argp_program_version = "0.3.0";
const char *argp_program_bug_address = "contact@mofangheavyindustries.com";

int frequency = 44100;

std::string server = "localhost";
unsigned int port = 1883;
std::string topic = "";
std::string alsaDevice = "";
std::string uriprefix = "";
std::string username = "";
std::string password = "";

vector<string> preloads;

bool run = true;
bool verbose = false;

SampleManager manager;

void handle_signal(int s)
{
    run = false;
}

// Callback invoked when a channel finishes playback
void channelFinished(int channel)
{
    auto it = channelInfo.find(channel);
    if (it != channelInfo.end())
    {
        // Free the seek chunk if one was allocated
        if (it->second.seekChunk != NULL)
        {
            // Only delete the chunk structure, not the audio buffer
            // (allocated = 0 means the buffer is not owned by this chunk)
            delete it->second.seekChunk;
        }
        channelInfo.erase(it);
    }
}

void connect_callback(struct mosquitto *mosq, void *obj, int result)
{
    switch (result)
    {
    case 0:
        printf("Connected successfully.\n");
        mosquitto_subscribe(mosq, NULL, topic.c_str(), 0);
        return;
    case 1:
        fprintf(stderr, "Connection refused - unacceptable protocol version.\n");
        break;
    case 2:
        fprintf(stderr, "Connection refused - identifier rejected.\n");
        break;
    case 3:
        fprintf(stderr, "Connection refused - broker unavailable.\n");
        break;
    default:
        fprintf(stderr, "Unknown error in connect callback, rc=%d\n", result);
    }

    exit(EX_PROTOCOL);
}

void stopAll(bool alsoStopBgm)
{
    if (verbose)
    {
        printf("Stopping all sounds, %s background music.\n", alsoStopBgm ? "including" : "excluding");
    }

    Mix_HaltChannel(-1);
}

Sample *precacheSample(const char *file)
{
    std::string filename = file;
    if (uriprefix.length() > 0)
    {
        filename = uriprefix + filename;
    }
    if (verbose)
    {
        printf("Preloading sample '%s'\n", filename.c_str());
    }
    return manager.GetSample(filename.c_str());
}

int playSample(const char *file, bool loop, float volume, bool exclusive, bool isBgm, int maxPlayLength)
{
    if (volume < 0.0f)
    {
        volume = 0.0f;
    }
    if (volume > 1.0f)
    {
        volume = 1.0f;
    }

    if (verbose)
    {
        printf("Playing sound %s, %s %s, at volume %d%%%s\n", file, loop ? "looping" : "once", maxPlayLength == -1 ? "forever" : "for a limited time", (int)(volume * 100.0f), isBgm ? "as background music." : ".");
        if (maxPlayLength != -1)
        {
            printf("\tMax play length is %d ms.\n", maxPlayLength);
        }
    }

    if (exclusive)
    {
        stopAll(isBgm);
    }

    Sample *sample = precacheSample(file);
    if (sample != NULL)
    {
        int channel = Mix_PlayChannelTimed(-1, sample->chunk, loop ? -1 : 0, maxPlayLength);
        if (channel >= 0)
        {
            int mixVolume = (int)(((float)MIX_MAX_VOLUME) * volume);
            Mix_Volume(channel, mixVolume);

            // Track channel playback info for seek support
            ChannelPlaybackInfo info;
            info.file = file;
            info.sample = sample;
            info.volume = volume;
            info.loop = loop;
            info.maxPlayLength = maxPlayLength;
            info.seekChunk = NULL;
            channelInfo[channel] = info;
        }
        return channel;
    }
    else
    {
        printf("Error - could not load requested sample '%s'\n", file);
        return -1;
    }
}

// Find a channel that is currently playing the specified file
// Returns -1 if no channel is playing that file
int findChannelByFile(const char *file)
{
    for (const auto &entry : channelInfo)
    {
        if (entry.second.file == file && Mix_Playing(entry.first))
        {
            return entry.first;
        }
    }
    return -1;
}

bool seekChannel(int channel, int positionMs)
{
    // Validate channel is currently playing and tracked
    auto it = channelInfo.find(channel);
    if (it == channelInfo.end())
    {
        fprintf(stderr, "Seek error: Channel %d is not currently tracked.\n", channel);
        return false;
    }

    if (!Mix_Playing(channel))
    {
        fprintf(stderr, "Seek error: Channel %d is not currently playing.\n", channel);
        return false;
    }

    ChannelPlaybackInfo &info = it->second;
    Sample *sample = info.sample;

    if (!sample || !sample->isValid())
    {
        fprintf(stderr, "Seek error: Invalid sample for channel %d.\n", channel);
        return false;
    }

    // Get audio format info to calculate byte offset
    int freq, channels;
    Uint16 format;
    Mix_QuerySpec(&freq, &format, &channels);

    // Calculate bytes per sample frame
    int bytesPerSample = (format & 0xFF) / 8;  // Bits to bytes
    int bytesPerFrame = bytesPerSample * channels;

    // Calculate byte offset from milliseconds
    // bytes = (positionMs / 1000.0) * freq * bytesPerFrame
    Uint32 byteOffset = (Uint32)((positionMs / 1000.0) * freq * bytesPerFrame);

    // Ensure proper alignment for the audio format
    byteOffset = byteOffset - (byteOffset % bytesPerFrame);

    Mix_Chunk *originalChunk = sample->chunk;

    // Bounds check
    if (byteOffset >= originalChunk->alen)
    {
        fprintf(stderr, "Seek error: Position %d ms is beyond the end of the sample.\n", positionMs);
        return false;
    }

    if (verbose)
    {
        printf("Seeking channel %d to position %d ms (byte offset %u of %u).\n",
               channel, positionMs, byteOffset, originalChunk->alen);
    }

    // Free any previously allocated seek chunk for this channel
    if (info.seekChunk != NULL)
    {
        delete info.seekChunk;
        info.seekChunk = NULL;
    }

    // Stop current playback on this channel
    Mix_HaltChannel(channel);

    // Create a new Mix_Chunk that points to the offset position in the original audio
    Mix_Chunk *seekChunk = new Mix_Chunk();
    seekChunk->allocated = 0;  // We don't own the audio buffer
    seekChunk->abuf = originalChunk->abuf + byteOffset;
    seekChunk->alen = originalChunk->alen - byteOffset;
    seekChunk->volume = originalChunk->volume;

    // Store the seek chunk for later cleanup
    info.seekChunk = seekChunk;

    // Play the seek chunk on the same channel
    int newChannel = Mix_PlayChannelTimed(channel, seekChunk, info.loop ? -1 : 0, info.maxPlayLength);

    if (newChannel < 0)
    {
        fprintf(stderr, "Seek error: Failed to play from seek position: %s\n", Mix_GetError());
        delete seekChunk;
        info.seekChunk = NULL;
        return false;
    }

    // Restore volume
    int mixVolume = (int)(((float)MIX_MAX_VOLUME) * info.volume);
    Mix_Volume(newChannel, mixVolume);

    // Update tracking (channel might be the same, but update to be safe)
    if (newChannel != channel)
    {
        channelInfo.erase(channel);
    }
    channelInfo[newChannel] = info;

    return true;
}

bool processCommand(Document &d)
{
    if (!d.IsObject())
    {
        fprintf(stderr, "Message is not a valid object.\n");
        return false;
    }

    if (!d.HasMember("command") || !d["command"].IsString())
    {
        fprintf(stderr, "Message does not have a 'command' property that is a string.\n");
        return false;
    }

    const char *command = d["command"].GetString();
    if (0 == strcasecmp(command, "soundPlay") || 0 == strcasecmp(command, "play"))
    {
        // Check to make sure we have a valid message first.
        if (!d.HasMember("message") || !d["message"].IsObject())
        {
            fprintf(stderr, "Message does not have a 'message' property that is an object.\n");
            return false;
        }

        if (!d["message"].HasMember("file") || !d["message"]["file"].IsString())
        {
            fprintf(stderr, "Message does does not have a 'message.file' property that is a string.\n");
            return false;
        }

        // Now, set up defaults...
        const char *file = d["message"]["file"].GetString();
        bool loop = false;
        float volume = 1.0f;
        bool exclusive = false;
        bool bgm = false;
        int maxPlayLength = -1;

        // And then update settings based on elements of the message.
        if (d["message"].HasMember("loop") && d["message"]["loop"].IsBool())
        {
            loop = d["message"]["loop"].GetBool();
        }

        if (d["message"].HasMember("volume") && d["message"]["volume"].IsFloat())
        {
            volume = d["message"]["volume"].GetFloat();
        }

        if (d["message"].HasMember("exclusive") && d["message"]["exclusive"].IsBool())
        {
            exclusive = d["message"]["exclusive"].GetBool();
        }

        if (d["message"].HasMember("bgm") && d["message"]["bgm"].IsBool())
        {
            bgm = d["message"]["bgm"].GetBool();
        }

        if (d["message"].HasMember("maxPlayLength") && d["message"]["maxPlayLength"].IsInt())
        {
            maxPlayLength = d["message"]["maxPlayLength"].GetInt();
        }

        playSample(file, loop, volume, exclusive, bgm, maxPlayLength);
        return true;
    }
    else if (0 == strcasecmp(command, "soundStopAll") || 0 == strcasecmp(command, "stopall"))
    {
        stopAll(true);
        return true;
    }
    else if (0 == strcasecmp(command, "soundFadeOut") || 0 == strcasecmp(command, "fadeout"))
    {
        if (d["message"].HasMember("time"))
        {
            int time = d["message"]["time"].GetInt();
            if (verbose)
            {
                printf("Fading out all channels for %d milliseconds.\n", time);
            }
            Mix_FadeOutChannel(-1, time);
        }
        return true;
    }
    else if (0 == strcasecmp(command, "soundPrecache") || 0 == strcasecmp(command, "precache"))
    {
        if (!d.HasMember("message") || !d["message"].IsObject())
        {
            fprintf(stderr, "Message does not have a 'message' property that is an object.\n");
            return false;
        }

        if (!d["message"].HasMember("file") || !d["message"]["file"].IsString())
        {
            fprintf(stderr, "Message does does not have a 'message.file' property that is a string.\n");
            return false;
        }

        const char *file = d["message"]["file"].GetString();
        precacheSample(file);

        if (verbose)
        {
            printf("Precached sound file '%s'.\n", file);
        }
        return true;
    }
    else if (0 == strcasecmp(command, "soundSeek") || 0 == strcasecmp(command, "seek"))
    {
        // Seek command requires a message object with file and position
        if (!d.HasMember("message") || !d["message"].IsObject())
        {
            fprintf(stderr, "Seek: Message does not have a 'message' property that is an object.\n");
            return false;
        }

        if (!d["message"].HasMember("file") || !d["message"]["file"].IsString())
        {
            fprintf(stderr, "Seek: Message does not have a 'message.file' property that is a string.\n");
            return false;
        }

        if (!d["message"].HasMember("position") || !d["message"]["position"].IsInt())
        {
            fprintf(stderr, "Seek: Message does not have a 'message.position' property that is an integer.\n");
            return false;
        }

        const char *file = d["message"]["file"].GetString();
        int position = d["message"]["position"].GetInt();

        if (position < 0)
        {
            fprintf(stderr, "Seek: Position must be non-negative.\n");
            return false;
        }

        int channel = findChannelByFile(file);
        if (channel < 0)
        {
            fprintf(stderr, "Seek: No channel is currently playing file '%s'.\n", file);
            return false;
        }

        return seekChannel(channel, position);
    }
    return false;
}

void message_callback(struct mosquitto *mosq, void *obj, const struct mosquitto_message *message)
{
    bool match = 0;
    mosquitto_topic_matches_sub(topic.c_str(), message->topic, &match);

    if (match)
    {
        Document d;
        d.Parse((const char *)message->payload);

        if (!processCommand(d))
        {
            fprintf(stderr, "Failed to process command '%s'.\n", (const char *)message->payload);
        }
    }
}

// Initializes the application data
bool initSDLAudio(void)
{
    SDL_Init(SDL_INIT_AUDIO);
    atexit(SDL_Quit);

    // load support for the OGG and MOD sample/music formats
    int flags = MIX_INIT_OGG | MIX_INIT_MOD | MIX_INIT_MP3;
    int initted = Mix_Init(flags);
    if (initted & flags != flags)
    {
        fprintf(stderr, "Mix_Init: Failed to init required ogg and mod support!\n");
        fprintf(stderr, "Mix_Init: %s\n", Mix_GetError());
        // handle error
        return false;
    }

    // Set up the audio stream
    int result = Mix_OpenAudio(frequency, AUDIO_S16SYS, 2, 512);
    if (result < 0)
    {
        fprintf(stderr, "Unable to open audio: %s\n", SDL_GetError());
        return false;
    }

    result = Mix_AllocateChannels(16);
    if (result < 0)
    {
        fprintf(stderr, "Unable to allocate mixing channels: %s\n", SDL_GetError());
        return false;
    }

    // Register channel finished callback for cleanup of seek chunks
    Mix_ChannelFinished(channelFinished);

    // set up HTTP/CURL library
    result = SDL_RWHttpInit();
    if (result != 0)
    {
        fprintf(stderr, "Unable to initialize web download library (%s).\n", result);
        return false;
    }

    return true;
}

static int parse_opt(int key, char *arg, struct argp_state *state)
{
    switch (key)
    {
    case 's':
        if (arg != NULL && *arg != '\0')
        {
            printf("Setting MQTT server to '%s'\n", arg);
            server = arg;
        }
        else
        {
            argp_error(state, "no server specified");
        }

        break;

    case 'p':
        if (arg != NULL)
        {
            port = atoi(arg);
            printf("Setting MQTT port to %d\n", port);
        }
        break;

    case 'u':
        if (arg != NULL && *arg != '\0')
        {
            printf("Setting URI prefix to '%s'\n", arg);
            uriprefix = arg;
        }
        break;

    case 200: //preload 
        if (arg != NULL && *arg != '\0')
        {
            printf("Preloading '%s'...\n", arg);
            preloads.push_back(arg);
        }
        break;
    case 'd':
        if (arg != NULL && *arg != '\0')
        {
            printf("Setting output device to ALSA PCM device '%s'\n", arg);
            setenv("SDL_AUDIODRIVER", "ALSA", true);
            setenv("AUDIODEV", arg, true);
        }
        else
        {
            argp_error(state, "no ALSA PCM device specified");
        }

        break;

    case 't':
        if (arg != NULL && *arg != '\0')
        {
            printf("Setting MQTT topic to '%s'\n", arg);
            topic = arg;
        }
        else
        {
            argp_error(state, "no topic specified");
        }
        break;

    case 'U':
        if (arg != NULL && *arg != '\0')
        {
            printf("Setting MQTT username to '%s'\n", arg);
            username = arg;
        }
        break;

    case 'P':
        if (arg != NULL && *arg != '\0')
        {
            printf("Setting MQTT password\n");
            password = arg;
        }
        break;

    case 'l':
        listAlsaDevices("pcm");
        exit(0);
        break;

    case 'v':
        printf("Verbose mode enabled.\n");
        verbose = true;
        break;

    case 'f':
        if (arg != NULL && *arg != '\0')
        {
            frequency = atoi(arg);
            printf("Setting frequency to %d Hz.\n", frequency);
        }

    case ARGP_KEY_NO_ARGS:
        if (topic.empty())
        {
            argp_usage(state);
        }
        break;
    }
    return 0;
}

int main(int argc, char **argv)
{
    printf("mqttaudio %s - %s %s\n", argp_program_version, __DATE__, __TIME__);
    printf("Copyright © 2016-2021 Mo Fang Heavy Industries LLC.  All rights reserved.\n\n");

    struct argp_option options[] =
        {
            {"server", 's', "server", 0, "The MQTT server to connect to (default localhost)"},
            {"port", 'p', "port", 0, "The MQTT server port (default 1883)"},
            {"topic", 't', "topic", 0, "The MQTT server topic to subscribe to (wildcards allowed)"},
            {"username", 'U', "username", 0, "The MQTT username for authentication (optional)"},
            {"password", 'P', "password", 0, "The MQTT password for authentication (optional)"},
            {"alsa-device", 'd', "pcm", 0, "The ALSA PCM device to use (setting this option overrides the SDL_AUDIODRIVER and AUDIODEV environment variables)"},
            {"list-devices", 'l', 0, 0, "Lists available ALSA PCM devices for the 'd' switch."},
            {"verbose", 'v', 0, 0, "Writes logging information for every sound played to stdout."},
            {"frequency", 'f', "frequency_in_khz", 0, "Sets the frequency for the sound output."},
            {"uri-prefix", 'u', "prefix", 0, "Sets a prefix to be prepended to all sound file locations."},
            {"preload", 200, "url", 0, "Preloads a sound sample on startup."},
            {0}};

    struct argp argp = {options, parse_opt};

    int retval = argp_parse(&argp, argc, argv, 0, 0, 0);
    if (retval != 0)
    {
        return retval;
    }

    // Initialize the SDL library
    printf("Initializing SDL library.\n");
    if (!initSDLAudio())
    {
        return 1;
    }

    // Precache audio samples
    for (auto& preload : preloads)
    {
        precacheSample(preload.c_str());
    }


    // Connect to the MQTT server
    uint8_t reconnect = true;
    char clientid[128];
    struct mosquitto *mosq;
    int rc = 0;

    // Intercept SIGINT and SIGTERM so we can exit the MQTT loop when they occur.
    signal(SIGINT, handle_signal);
    signal(SIGTERM, handle_signal);

    mosquitto_lib_init();

    memset(clientid, 0, 128);
    snprintf(clientid, 127, "mqttaudio_%d", getpid());
    mosq = mosquitto_new(clientid, true, 0);

    if (mosq)
    {
        mosquitto_connect_callback_set(mosq, connect_callback);
        mosquitto_message_callback_set(mosq, message_callback);

        // Set username and password if provided
        if (!username.empty() || !password.empty())
        {
            rc = mosquitto_username_pw_set(mosq,
                username.empty() ? NULL : username.c_str(),
                password.empty() ? NULL : password.c_str());
            if (rc != MOSQ_ERR_SUCCESS)
            {
                fprintf(stderr, "Failed to set username/password: %d\n", rc);
                return EX_CONFIG;
            }
        }

        printf("Connecting to server %s\n", server.c_str());
        rc = mosquitto_connect(mosq, server.c_str(), port, 60);
        if (MOSQ_ERR_SUCCESS != rc)
        {
            fprintf(stderr, "Failed to connect to server %s (%d) - ", server.c_str(), rc);
            return EX_UNAVAILABLE;
        }

        while (run)
        {
            rc = mosquitto_loop(mosq, -1, 1);
            if (run && rc)
            {
                fprintf(stderr, "Server connection lost to server %s;  attempting to reconnect.\n", server.c_str());
                sleep(10);
                rc = mosquitto_reconnect(mosq);
                if (MOSQ_ERR_SUCCESS != rc)
                {
                    fprintf(stderr, "Failed to reconnect to server %s (%d) \n", server.c_str(), rc);
                }
                else
                {
                    fprintf(stderr, "Reconnected to server %s (%d) \n", server.c_str(), rc);
                    mosquitto_subscribe(mosq, NULL, topic.c_str(), 0);
                }
                
            }
        }

        mosquitto_destroy(mosq);
    }

    printf("Exiting mofang_player...\n");

    printf("Cleaning up MQTT connection...\n");
    mosquitto_lib_cleanup();

    printf("Cleaning up audio samples...\n");
    manager.FreeAll();

    printf("Closing audio device...\n");
    Mix_CloseAudio();
    SDL_RWHttpShutdown();
    SDL_Quit();

    printf("Cleanup complete.\n");
    return 0;
}
