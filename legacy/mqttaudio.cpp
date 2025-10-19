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

const char *argp_program_version = "0.1.1";
const char *argp_program_bug_address = "contact@mofangheavyindustries.com";

int frequency = 44100;

std::string server = "localhost";
unsigned int port = 1883;
std::string topic = "";
std::string alsaDevice = "";
std::string uriprefix = "";

vector<string> preloads;

bool run = true;
bool verbose = false;

SampleManager manager;

void handle_signal(int s)
{
    run = false;
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

void playSample(const char *file, bool loop, float volume, bool exclusive, bool isBgm, int maxPlayLength)
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
        int mixVolume = (int)(((float)MIX_MAX_VOLUME) * volume);
        Mix_Volume(channel, mixVolume);
    }
    else
    {
        printf("Error - could not load requested sample '%s'\n", file);
    }
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
