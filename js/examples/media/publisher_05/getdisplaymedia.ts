import { initialize } from './core';
import {
  //setUpStartGetUserMediaButton,
  setUpStartGetDisplayMediaButton,
  //setUpLoadFileButton,
  setUpPlayButton,
  //setUpPauseButton,
  setUpConnectButton
} from './ui';

async function main() {
  await initialize();

  //setUpStartGetUserMediaButton();
  setUpStartGetDisplayMediaButton();
  //setUpLoadFileButton();
  setUpPlayButton();
  //setUpPauseButton();
  setUpConnectButton();
}

main();