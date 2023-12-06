import init, {MOQTClient} from './pkg/moqt_client_sample';

init().then(async () => {
    console.log('init wasm-pack');

    const client = new MOQTClient("https://localhost:4433");
    console.log(client.id, client);
    console.log("URL:", client.url());

    client.set_and_exec_callback((a,b) => {
      console.log(a + b);
      console.log(a - b);
      console.log(a * b);
      console.log(a / b);
    });

    client.set_and_exec_callback((a,b) => {
      console.log(a, b);
    });

    console.log("URL:", client.url());

    client.onSetupCallback(async (serverSetupMessage) => {
      console.log({serverSetupMessage});
      await client.sendAnnounceMessage('this is track namespace');
    });

    client.onAnnounceCallback(async (announceResponse) => {
      console.log({announceResponse});

      await client.sendSubscribeMessage('this is track namespace', 'this is track name');

      await client.sendUnsubscribeMessage('this is track namespace', 'this is track name');

      await client.sendUnannounceMessage('this is track namespace');
    });

    client.onSubscribeCallback(async (subscribeResponse) => {
      console.log({subscribeResponse});
    });

    await client.start();

    client.sendSetupMessage().then(() => {
      console.log("sendSetupMessage");
    });
});
