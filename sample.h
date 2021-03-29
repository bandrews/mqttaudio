#ifndef SAMPLE_H
#define SAMPLE_H

#include <string>
using namespace std;

#include "SDL.h"
#include "SDL_mixer.h"

#define HTTP_PROTOCOL_PREFIX "http"

class Sample
{
public:
    std::string sourceUri;
    Sample(const char *uri);

    bool isValid();
    Mix_Chunk *chunk = NULL;

    void Free();

private:
    void initSample();
};

#endif