#include "samplemanager.h"

Sample* SampleManager::GetSample(const char * uri)
{
    if (_database.find(uri) != _database.end())
    {
        return _database[uri];
    }
    else
    {
        Sample* sample = new Sample(uri);
        if (sample->isValid())
        {
            std::string key = uri;
            _database.insert({key, sample});
            return sample;
        }
        else
        {
        return 0;
        }
    }
}

void SampleManager::FreeAll()
{
    for( const auto& s : _database ) {
        s.second->Free();
    }
}