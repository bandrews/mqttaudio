#!/usr/bin/env node
// ABOUTME: Interactive integration test for HTTP REST API.
// ABOUTME: Walks user through testing each endpoint with audio confirmation.

const http = require('http');
const { spawn, execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const readline = require('readline');

// Configuration
const HTTP_PORT = process.env.HTTP_PORT || 8765;
const HTTP_HOST = process.env.HTTP_HOST || '127.0.0.1';
const BASE_URL = `http://${HTTP_HOST}:${HTTP_PORT}`;
const PROJECT_ROOT = path.resolve(__dirname, '../..');
const TEST_AUDIO_DIR = path.join(PROJECT_ROOT, 'tests/audio');
const MQTTAUDIO_BIN = path.join(PROJECT_ROOT, 'target/release/mqttaudio');

// Colors for terminal output
const colors = {
  red: '\x1b[31m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  blue: '\x1b[34m',
  reset: '\x1b[0m',
};

// Test state
let mqttaudioProcess = null;
let passedTests = 0;
let failedTests = 0;

// Readline interface for user input
const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
});

// Helper functions
function printHeader(text) {
  console.log('');
  console.log(`${colors.blue}════════════════════════════════════════════════════════════${colors.reset}`);
  console.log(`${colors.blue}  ${text}${colors.reset}`);
  console.log(`${colors.blue}════════════════════════════════════════════════════════════${colors.reset}`);
}

function printTest(text) {
  console.log('');
  console.log(`${colors.yellow}▶ TEST: ${text}${colors.reset}`);
}

function printExpect(text) {
  console.log(`${colors.green}  Expected: ${text}${colors.reset}`);
}

function printCommand(text) {
  console.log(`  Command: ${colors.blue}${text}${colors.reset}`);
}

function printPass(text) {
  console.log(`${colors.green}  ✓ ${text}${colors.reset}`);
  passedTests++;
}

function printFail(text) {
  console.log(`${colors.red}  ✗ ${text}${colors.reset}`);
  failedTests++;
}

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function askQuestion(question) {
  return new Promise(resolve => {
    rl.question(question, resolve);
  });
}

async function askConfirmation() {
  const response = await askQuestion('\n  Did this work correctly? [Y/n/q] ');
  const lower = response.toLowerCase().trim();

  if (lower === 'q') {
    console.log('Quitting...');
    await cleanup();
    process.exit(0);
  } else if (lower === 'n') {
    printFail('Test failed');
  } else {
    printPass('Test passed');
  }
}

async function waitKey() {
  await askQuestion('\n  Press Enter to continue...');
}

// HTTP helper
function httpRequest(method, urlPath, body = null) {
  return new Promise((resolve, reject) => {
    const url = new URL(urlPath, BASE_URL);
    const options = {
      hostname: url.hostname,
      port: url.port,
      path: url.pathname + url.search,
      method: method,
      headers: {},
    };

    if (body) {
      const data = JSON.stringify(body);
      options.headers['Content-Type'] = 'application/json';
      options.headers['Content-Length'] = Buffer.byteLength(data);
    }

    const req = http.request(options, res => {
      let data = '';
      res.on('data', chunk => data += chunk);
      res.on('end', () => {
        try {
          resolve({ status: res.statusCode, data: JSON.parse(data) });
        } catch {
          resolve({ status: res.statusCode, data: data });
        }
      });
    });

    req.on('error', reject);

    if (body) {
      req.write(JSON.stringify(body));
    }
    req.end();
  });
}

async function get(urlPath) {
  return httpRequest('GET', urlPath);
}

async function post(urlPath, body = {}) {
  return httpRequest('POST', urlPath, body);
}

// Cleanup
async function cleanup() {
  console.log('\nCleaning up...');
  if (mqttaudioProcess) {
    mqttaudioProcess.kill('SIGTERM');
    await sleep(500);
    if (!mqttaudioProcess.killed) {
      mqttaudioProcess.kill('SIGKILL');
    }
  }
  rl.close();
  console.log('Done.');
}

// Setup functions
function checkTestAudio() {
  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  const testFile5s = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');

  if (!fs.existsSync(TEST_AUDIO_DIR)) {
    fs.mkdirSync(TEST_AUDIO_DIR, { recursive: true });
  }

  if (!fs.existsSync(testFile)) {
    console.log('Creating test audio file (2s)...');
    try {
      execSync(`ffmpeg -f lavfi -i "sine=frequency=440:duration=2" -ar 44100 -ac 2 -y "${testFile}"`,
        { stdio: 'pipe' });
    } catch (e) {
      console.log(`${colors.red}Warning: Could not create test audio with ffmpeg.${colors.reset}`);
      console.log('Please ensure ffmpeg is installed or create test audio manually.');
      process.exit(1);
    }
  }

  if (!fs.existsSync(testFile5s)) {
    console.log('Creating test audio file (5s)...');
    try {
      execSync(`ffmpeg -f lavfi -i "sine=frequency=880:duration=5" -ar 44100 -ac 2 -y "${testFile5s}"`,
        { stdio: 'pipe' });
    } catch (e) {
      console.log(`${colors.yellow}Warning: Could not create 5s test audio.${colors.reset}`);
    }
  }
}

function buildProject() {
  printHeader('Building mqttaudio (release)');
  try {
    execSync('cargo build --release', { cwd: PROJECT_ROOT, stdio: 'inherit' });
    if (!fs.existsSync(MQTTAUDIO_BIN)) {
      console.log(`${colors.red}Build failed - binary not found${colors.reset}`);
      process.exit(1);
    }
    console.log(`${colors.green}Build successful${colors.reset}`);
  } catch (e) {
    console.log(`${colors.red}Build failed${colors.reset}`);
    process.exit(1);
  }
}

async function startServer() {
  printHeader('Starting mqttaudio in HTTP-only mode');
  console.log(`  Port: ${HTTP_PORT}`);

  return new Promise((resolve, reject) => {
    mqttaudioProcess = spawn(MQTTAUDIO_BIN, ['--http-port', String(HTTP_PORT)], {
      cwd: PROJECT_ROOT,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    mqttaudioProcess.stdout.on('data', data => {
      if (process.env.VERBOSE) {
        console.log(`  [mqttaudio] ${data.toString().trim()}`);
      }
    });

    mqttaudioProcess.stderr.on('data', data => {
      if (process.env.VERBOSE) {
        console.log(`  [mqttaudio stderr] ${data.toString().trim()}`);
      }
    });

    mqttaudioProcess.on('error', err => {
      console.log(`${colors.red}Failed to start server: ${err.message}${colors.reset}`);
      reject(err);
    });

    // Wait for server to start
    console.log('  Waiting for server to start...');

    const checkServer = async (attempts = 0) => {
      if (attempts > 20) {
        console.log(`${colors.red}Server failed to start after 10 seconds${colors.reset}`);
        reject(new Error('Server timeout'));
        return;
      }

      try {
        await get('/health');
        console.log(`${colors.green}Server started successfully (PID: ${mqttaudioProcess.pid})${colors.reset}`);
        resolve();
      } catch {
        await sleep(500);
        checkServer(attempts + 1);
      }
    };

    checkServer();
  });
}

// Test functions
async function testHealthEndpoint() {
  printTest('Health Endpoint');
  printCommand(`curl ${BASE_URL}/health`);

  try {
    const { data } = await get('/health');
    console.log(`  Response: ${JSON.stringify(data)}`);

    if (data.status === 'ok') {
      printPass('Health endpoint working');
    } else {
      printFail('Unexpected response');
    }
  } catch (e) {
    printFail(`Request failed: ${e.message}`);
  }
}

async function testStatusEndpoint() {
  printTest('Status Endpoint');
  printCommand(`curl ${BASE_URL}/status`);

  try {
    const { data } = await get('/status');
    console.log(`  Response: ${JSON.stringify(data)}`);

    if (data.status === 'running') {
      printPass('Status endpoint working');
    } else {
      printFail('Unexpected response');
    }
  } catch (e) {
    printFail(`Request failed: ${e.message}`);
  }
}

async function testPlayBasic() {
  printTest('Basic Play Command');
  printExpect('You should hear a 440Hz tone for 2 seconds');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  printCommand(`POST ${BASE_URL}/play with file: ${testFile}`);

  await post('/play', { file: testFile });
  await sleep(2500);
  await askConfirmation();
}

async function testPlayWithVolume() {
  printTest('Play with Volume');
  printExpect('You should hear a QUIET 440Hz tone (volume at 30%)');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  await post('/play', { file: testFile, volume: 0.3 });
  await sleep(2500);
  await askConfirmation();
}

async function testPlayWithFadeIn() {
  printTest('Play with Fade In');
  printExpect('You should hear a tone that fades in over 1 second');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  await post('/play', { file: testFile, fade_in: 1000 });
  await sleep(2500);
  await askConfirmation();
}

async function testPlayWithVoice() {
  printTest('Play with Voice Grouping');
  printExpect('Two sounds should play simultaneously on voice "test_voice"');

  const testFile1 = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  const testFile2 = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');

  await post('/play', { file: testFile1, voice: 'test_voice' });
  await post('/play', { file: testFile2, voice: 'test_voice', volume: 0.5 });
  await sleep(3000);
  await askConfirmation();

  // Stop the voice
  await post('/voice/stop', { voice: 'test_voice' });
}

async function testStopall() {
  printTest('Stop All Command');
  printExpect('Playing a 5-second sound, then stopping it after 1 second');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');
  await post('/play', { file: testFile });
  await sleep(1000);

  printCommand(`POST ${BASE_URL}/stopall`);
  await post('/stopall');

  console.log('  The sound should have stopped abruptly');
  await askConfirmation();
}

async function testStopWithFade() {
  printTest('Stop with Fade Out');
  printExpect('Playing a 5-second sound, then fading it out over 1 second');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');
  await post('/play', { file: testFile, voice: 'fade_test' });
  await sleep(1000);

  printCommand(`POST ${BASE_URL}/voice/fade_out`);
  await post('/voice/fade_out', { voice: 'fade_test', time_ms: 1000 });
  await sleep(1500);

  console.log('  The sound should have faded out smoothly');
  await askConfirmation();
}

async function testVolumeChange() {
  printTest('Volume Change');
  printExpect('Playing sound, then changing volume from 100% to 20%');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');
  await post('/play', { file: testFile, voice: 'vol_test' });
  await sleep(1000);

  console.log('  Reducing volume to 20%...');
  await post('/voice/volume', { voice: 'vol_test', volume: 0.2 });
  await sleep(2000);

  await post('/stopall');
  console.log('  The sound should have gotten quieter');
  await askConfirmation();
}

async function testLooping() {
  printTest('Looping Playback');
  printExpect('Playing a 2-second sound in a loop (will play for ~5 seconds)');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  await post('/play', { file: testFile, loop: true, voice: 'loop_test' });

  console.log('  Listening for 5 seconds...');
  await sleep(5000);

  await post('/stopall');
  console.log('  You should have heard the sound repeat at least twice');
  await askConfirmation();
}

async function testGenericCommand() {
  printTest('Generic /command Endpoint');
  printExpect('Using /command to play a sound (same JSON as MQTT)');
  printCommand(`POST ${BASE_URL}/command with MQTT-style JSON`);

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  await post('/command', {
    command: 'play',
    message: { file: testFile }
  });

  await sleep(2500);
  await askConfirmation();
}

async function testStatusSamples() {
  printTest('Status/Samples Endpoint (while playing)');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');
  await post('/play', { file: testFile, voice: 'status_test' });
  await sleep(500);

  printCommand(`GET ${BASE_URL}/status/samples`);
  const { data } = await get('/status/samples');
  console.log(`  Response: ${JSON.stringify(data)}`);

  await post('/stopall');

  if (Array.isArray(data.samples)) {
    printPass('Shows playing samples');
  } else {
    printFail('Unexpected response');
  }
}

async function testStatusVoices() {
  printTest('Status/Voices Endpoint');
  printCommand(`GET ${BASE_URL}/status/voices`);

  const { data } = await get('/status/voices');
  console.log(`  Response: ${JSON.stringify(data)}`);

  if (Array.isArray(data.voices)) {
    printPass('Voices endpoint working');
  } else {
    printFail('Unexpected response');
  }
}

async function testStatusCache() {
  printTest('Status/Cache Endpoint');
  printCommand(`GET ${BASE_URL}/status/cache`);

  const { data } = await get('/status/cache');
  console.log(`  Response: ${JSON.stringify(data)}`);

  if (data.memory && data.disk) {
    printPass('Cache endpoint working');
  } else {
    printFail('Unexpected response');
  }
}

async function testPrecache() {
  printTest('Precache Command');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  printCommand(`POST ${BASE_URL}/precache`);

  const { data } = await post('/precache', { file: testFile });
  console.log(`  Response: ${JSON.stringify(data)}`);

  await sleep(1000);

  if (data.success) {
    printPass('Precache command accepted');
  } else {
    printFail('Unexpected response');
  }
}

async function testCacheClear() {
  printTest('Cache Clear Command');
  printCommand(`POST ${BASE_URL}/cache/clear`);

  const { data } = await post('/cache/clear');
  console.log(`  Response: ${JSON.stringify(data)}`);

  if (data.success) {
    printPass('Cache clear command accepted');
  } else {
    printFail('Unexpected response');
  }
}

// ============================================================================
// Streaming and Performance Tests
// ============================================================================

async function testColdLoadTiming() {
  printTest('Cold Load Timing');
  printExpect('Clear cache, then measure time to first sound');

  // Clear all caches first
  await post('/cache/clear');
  await sleep(100);

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');
  console.log('  Clearing cache and playing cold...');

  const startTime = Date.now();
  await post('/play', { file: testFile, voice: 'cold_test' });

  // The command returns immediately - measure API response time
  const apiTime = Date.now() - startTime;
  console.log(`  API response time: ${apiTime}ms`);

  await sleep(1000);
  await post('/stopall');

  console.log('  You should have heard the sound start quickly (< 100ms target)');
  await askConfirmation();
}

async function testHotLoadTiming() {
  printTest('Hot Load Timing (Cache Hit)');
  printExpect('Load file into cache, clear, reload - should be nearly instant');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');

  // Ensure file is cached
  await post('/precache', { file: testFile });
  await sleep(500);

  console.log('  File is now cached. Playing from cache...');

  const startTime = Date.now();
  await post('/play', { file: testFile, voice: 'hot_test' });
  const apiTime = Date.now() - startTime;
  console.log(`  API response time: ${apiTime}ms (should be very fast)`);

  await sleep(1000);
  await post('/stopall');

  console.log('  Sound should have started essentially instantly');
  await askConfirmation();
}

async function testPrecacheThenPlay() {
  printTest('Precache Then Play');
  printExpect('Precache file, then play should be instant');

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');

  // Clear cache first
  await post('/cache/clear');
  await sleep(100);

  // Precache the file
  console.log('  Precaching file...');
  const precacheStart = Date.now();
  await post('/precache', { file: testFile });
  await sleep(500); // Give it time to load

  const { data: cacheStatus } = await get('/status/cache');
  console.log(`  Cache status: ${JSON.stringify(cacheStatus.memory)}`);

  // Now play should be instant
  console.log('  Playing precached file...');
  const playStart = Date.now();
  await post('/play', { file: testFile, voice: 'precache_test' });
  const playTime = Date.now() - playStart;
  console.log(`  Play response time: ${playTime}ms`);

  await sleep(2500);

  console.log('  Sound should have started immediately (file was precached)');
  await askConfirmation();
}

async function testPlayDuringPreload() {
  printTest('Play During Preload');
  printExpect('Start precache, then immediately play same file');

  // Clear cache first
  await post('/cache/clear');
  await sleep(100);

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');

  // Start precaching
  console.log('  Starting precache...');
  post('/precache', { file: testFile }); // Don't await

  // Immediately try to play the same file
  console.log('  Immediately playing same file...');
  await post('/play', { file: testFile, voice: 'during_preload' });

  await sleep(2000);
  await post('/stopall');

  console.log('  Sound should have played (sharing the same loading buffer)');
  await askConfirmation();
}

async function testStopDuringPreload() {
  printTest('Stop During Preload');
  printExpect('Start playing file, stop it before fully loaded');

  // Clear cache first
  await post('/cache/clear');
  await sleep(100);

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');

  console.log('  Playing file (cold load)...');
  await post('/play', { file: testFile, voice: 'stop_preload_test' });

  // Stop immediately
  console.log('  Stopping immediately...');
  await sleep(100); // Give it just a moment
  await post('/voice/stop', { voice: 'stop_preload_test' });

  await sleep(500);

  console.log('  Sound should have stopped cleanly (no crash, no stuck audio)');
  await askConfirmation();
}

async function testMultipleConcurrentLoads() {
  printTest('Multiple Concurrent Loads');
  printExpect('Play multiple different files simultaneously (cold load)');

  // Clear cache first
  await post('/cache/clear');
  await sleep(100);

  const testFile1 = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');
  const testFile2 = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');
  const testFile3 = path.join(TEST_AUDIO_DIR, 'music_200hz.wav');

  console.log('  Playing 3 files simultaneously (cold load)...');
  await Promise.all([
    post('/play', { file: testFile1, voice: 'concurrent1', volume: 0.5 }),
    post('/play', { file: testFile2, voice: 'concurrent2', volume: 0.5 }),
    post('/play', { file: testFile3, voice: 'concurrent3', volume: 0.3 }),
  ]);

  await sleep(3000);
  await post('/stopall');

  console.log('  All 3 sounds should have played together');
  await askConfirmation();
}

async function testCacheMemoryStatus() {
  printTest('Cache Memory Status');
  printExpect('Load files and check memory usage reporting');

  // Clear cache first
  await post('/cache/clear');
  await sleep(100);

  const { data: before } = await get('/status/cache');
  console.log(`  Before: ${JSON.stringify(before.memory)}`);

  // Load some files
  const testFile = path.join(TEST_AUDIO_DIR, 'music_200hz.wav'); // Larger file
  await post('/precache', { file: testFile });
  await sleep(1000);

  const { data: after } = await get('/status/cache');
  console.log(`  After: ${JSON.stringify(after.memory)}`);

  // Field is "entries" not "entry_count"
  if (after.memory.entries > before.memory.entries) {
    printPass('Memory cache shows entries');
  } else {
    printFail('Memory cache not reporting correctly');
  }
}

async function testPrecacheDoesNotPlay() {
  printTest('Precache Does Not Play Audio');
  printExpect('Precache a file - should load silently without any audio output');

  // Clear cache first
  await post('/cache/clear');
  await sleep(100);

  const testFile = path.join(TEST_AUDIO_DIR, 'test_440hz_2s.wav');

  console.log('  Precaching file (you should hear NOTHING)...');
  await post('/precache', { file: testFile });

  // Wait for precache to complete
  await sleep(1500);

  // Verify it's cached
  const { data: cacheStatus } = await get('/status/cache');
  console.log(`  Cache status: ${cacheStatus.memory.entries} entries, ${cacheStatus.memory.size_mb.toFixed(2)} MB`);

  // Verify no samples are playing
  const { data: samples } = await get('/status/samples');
  console.log(`  Active samples: ${samples.samples ? samples.samples.length : 0}`);

  if (cacheStatus.memory.entries > 0 && (!samples.samples || samples.samples.length === 0)) {
    console.log('  File is cached but not playing - correct behavior');
  } else {
    console.log('  WARNING: Either not cached or unexpectedly playing');
  }

  await askConfirmation();
}

async function testSeekDuringPlayback() {
  printTest('Seek During Playback');
  printExpect('Play a 5s file, seek to 4s - sound should stop naturally within ~1s');

  // Make sure nothing is playing
  await post('/stopall');
  await sleep(100);

  const testFile = path.join(TEST_AUDIO_DIR, 'test_beep_5s.wav');

  // Play the file (non-looping)
  console.log('  Starting 5-second file (non-looping)...');
  const { data: playData } = await post('/play', { file: testFile, voice: 'seek_test', loop: false });
  console.log(`  Play response: ${JSON.stringify(playData)}`);

  console.log('  Playing for 1 second...');
  await sleep(1000);

  // Get sample info before seek
  const { data: samplesBefore } = await get('/status/samples');
  console.log(`  Active samples before seek: ${samplesBefore.samples ? samplesBefore.samples.length : 0}`);

  if (samplesBefore.samples && samplesBefore.samples.length > 0) {
    const sample = samplesBefore.samples[0];
    console.log(`  Sample internal_id: ${sample.internal_id}, id: ${sample.id || 'none'}, position: ${sample.position_ms || 'unknown'}ms`);

    console.log('  Seeking to 4000ms (near end of 5s file)...');
    // Use internal_id for precise targeting (system-assigned unique ID)
    const { data: seekData } = await post('/seek', { internal_id: sample.internal_id, position_ms: 4000 });
    console.log(`  Seek response: ${JSON.stringify(seekData)}`);

    // Check position after seek
    await sleep(100);
    const { data: samplesAfter } = await get('/status/samples');
    if (samplesAfter.samples && samplesAfter.samples.length > 0) {
      const afterSample = samplesAfter.samples[0];
      console.log(`  Position after seek: ${afterSample.position_ms || 'unknown'}ms`);
    }

    console.log('  Waiting 2 seconds - sound should end naturally around 1s after seek...');
    await sleep(2000);

    // Check if still playing
    const { data: samplesFinal } = await get('/status/samples');
    const stillPlaying = samplesFinal.samples && samplesFinal.samples.length > 0;
    console.log(`  Still playing after 2s wait: ${stillPlaying}`);

    if (stillPlaying) {
      console.log('  WARNING: Sample still playing - seek may not have worked correctly');
    } else {
      console.log('  Sample ended naturally after seek - correct behavior');
    }
  } else {
    console.log('  No samples found to seek');
  }

  await post('/stopall');
  await askConfirmation();
}

// Main
async function main() {
  printHeader('mqttaudio HTTP Interactive Integration Test');
  console.log('');
  console.log('This test will walk you through testing the HTTP REST API.');
  console.log('You will need to listen and confirm that audio plays correctly.');
  console.log('');
  console.log('Make sure your audio output is working and at a reasonable volume.');
  console.log('');

  await askQuestion('Press Enter to begin...');

  try {
    // Setup
    checkTestAudio();
    buildProject();
    await startServer();

    // API endpoint tests (automated)
    printHeader('API Endpoint Tests (Automated)');
    await testHealthEndpoint();
    await testStatusEndpoint();
    await testStatusVoices();
    await testStatusCache();

    // Audio playback tests (manual confirmation)
    printHeader('Audio Playback Tests (Manual Confirmation)');
    await testPlayBasic();
    await testPlayWithVolume();
    await testPlayWithFadeIn();
    await testPlayWithVoice();
    await testLooping();
    await testStopall();
    await testStopWithFade();
    await testVolumeChange();
    await testGenericCommand();

    // Cache and status tests
    printHeader('Cache and Status Tests');
    await testPrecache();
    await testStatusSamples();
    await testCacheClear();

    // Streaming and performance tests
    printHeader('Streaming and Performance Tests');
    await testColdLoadTiming();
    await testHotLoadTiming();
    await testPrecacheThenPlay();
    await testPrecacheDoesNotPlay();
    await testPlayDuringPreload();
    await testStopDuringPreload();
    await testMultipleConcurrentLoads();
    await testCacheMemoryStatus();
    await testSeekDuringPlayback();

    // Summary
    printHeader('Test Summary');
    console.log('');
    console.log(`  ${colors.green}Passed: ${passedTests}${colors.reset}`);
    console.log(`  ${colors.red}Failed: ${failedTests}${colors.reset}`);
    console.log(`  Total:  ${passedTests + failedTests}`);
    console.log('');

    if (failedTests === 0) {
      console.log(`${colors.green}All tests passed!${colors.reset}`);
    } else {
      console.log(`${colors.red}Some tests failed. Please review the output above.${colors.reset}`);
    }

  } catch (e) {
    console.log(`${colors.red}Error: ${e.message}${colors.reset}`);
    if (process.env.VERBOSE) {
      console.log(e.stack);
    }
  } finally {
    await cleanup();
  }
}

// Handle Ctrl+C
process.on('SIGINT', async () => {
  console.log('\nInterrupted...');
  await cleanup();
  process.exit(0);
});

main();
