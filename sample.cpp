#include "sample.h"
#include "SDL_rwhttp.h"

Sample::Sample(const char *uri)
{
    sourceUri = uri;

    this->initSample();
}

bool Sample::isValid()
{
    return this->chunk != NULL;
}

void Sample::initSample()
{
    bool isWeb = strncmp(this->sourceUri.c_str(), HTTP_PROTOCOL_PREFIX, strlen(HTTP_PROTOCOL_PREFIX)) == 0;

    if (!isWeb)
    {
        this->chunk = Mix_LoadWAV(this->sourceUri.c_str());
    }
    else
    {
        auto source = SDL_RWFromHttpSync(this->sourceUri.c_str());
        printf("Retrieving %s from remote server...\n", this->sourceUri.c_str());
        this->chunk = Mix_LoadWAV_RW(source, true);
    }

    if (this->chunk == NULL)
    {
        fprintf(stderr, "Unable to load wave file: %s\n", this->sourceUri.c_str());
        fprintf(stderr, "err: %s\n", SDL_GetError());
    }
    else
    {
        printf("Loaded new sample %s successfully.\n", this->sourceUri.c_str());
    }
}

void Sample::Free()
{
    Mix_FreeChunk(this->chunk);
    this->chunk = NULL;
}