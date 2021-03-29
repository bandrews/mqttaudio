all:  mqttaudio

mqttaudio: mqttaudio.cpp sample.cpp sample.h samplemanager.h samplemanager.cpp SDL_rwhttp.c SDL_rwhttp.h
	g++ -o mqttaudio -I/usr/include/SDL2 -I/usr/include/alsa -L/usr/lib/ mqttaudio.cpp sample.cpp samplemanager.cpp SDL_rwhttp.c -Wl,--allow-shlib-undefined -lSDL2 -lSDL2_mixer -lrt -lmosquitto -lasound -lcurl -g

clean:
	rm mofang_player

install-dependencies:
	apt-get install libsdl2-dev libsdl2-mixer-dev libsdl2-net-dev libsdl2-ttf-dev libsdl2-image-dev libsdl2-gfx-dev mosquitto libmosquitto-dev mosquitto-clients libcurl4-openssl-dev