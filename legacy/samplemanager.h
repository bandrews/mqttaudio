#ifndef SAMPLEMANAGER_H
#define SAMPLEMANAGER_H

#include <string>
#include <unordered_map>

#include "sample.h"

using namespace std;

class SampleManager {
    public:
        Sample* GetSample(const char* uri);
        void FreeAll();

    private:
        std::unordered_map<std::string, Sample*> _database;

};

#endif