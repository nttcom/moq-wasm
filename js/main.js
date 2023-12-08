import init, {MOQTClient} from './pkg/moqt_client_sample';

init().then(async () => {
  console.log('init wasm-pack');

  const connectBtn = document.getElementById('connectBtn');
  connectBtn.addEventListener('click', async () => {
    const url = document.form.url.value;

    const client = new MOQTClient(url);
    console.log(client.id, client);
    console.log("URL:", client.url());

    const ary = new Uint8Array([1,1,2,3,5,8,13,21,34,55,89,144,233]);
    console.log(ary);
    client.array_buffer_sample_method(ary);
    client.array_buffer_sample_method(ary);

    client.onSetup(async (serverSetupMessage) => {
      console.log({serverSetupMessage});
    });

    client.onAnnounce(async (announceResponse) => {
      console.log({announceResponse});
    });

    client.onSubscribe(async (subscribeResponse) => {
      console.log('relay will want to subscribe');
      console.log({subscribeResponse});
    });

    client.onSubscribeResponse(async (subscribeResponse) => {
      console.log({subscribeResponse});
    });

    client.onObject(async (objectMessage) => {
      console.log({objectMessage});
    });

    const sendBtn = document.getElementById('sendBtn');

    const send = async () => {
      console.log('send btn clicked');
      const streamDatagram = Array.from(form['stream-datagram']).filter(elem => elem.checked)[0].value;
      const messageType = form['message-type'].value;
      const versions = form['versions'].value.split(',').map(BigInt);
      const role = Array.from(form['role']).filter(elem => elem.checked)[0].value;
      const isAddPath = !!form['add-path'].checked;

      console.log({streamDatagram,messageType,versions,role,isAddPath});

      const trackNamespace = 'this is track namespace';
      const trackName = 'this is track name';

      switch (messageType) {
        case 'setup':
          await client.sendSetupMessage(role, versions);
          break;
        case 'object':
          await client.sendObjectMessage(1n, 0n, 0n, 0n, new Uint8Array([0xde, 0xad, 0xbe, 0xef]));
          break;
        case 'object-wo-length':
          await client.sendObjectMessageWithoutLength(1n, 0n, 0n, 0n, new Uint8Array([0xde, 0xad, 0xbe, 0xef]));
          break;
        case 'announce':
          await client.sendAnnounceMessage(trackNamespace);
          break;
        case 'unannounce':
          await client.sendUnannounceMessage(trackNamespace);
          break;
        case 'subscribe':
          await client.sendSubscribeMessage(trackNamespace, trackName);
          break;
        case 'unsubscribe':
          await client.sendUnsubscribeMessage(trackNamespace, trackName);
          break;
      }
    };

    sendBtn.addEventListener('click', send);

    await client.start();
  });
});
