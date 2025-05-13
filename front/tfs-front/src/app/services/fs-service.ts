import {Injectable} from '@angular/core';
import {HttpClient} from '@angular/common/http';
import {AuthService} from './auth.service';
import {
  concat,
  concatMap,
  defer, EMPTY,
  flatMap,
  from,
  map, merge,
  mergeMap,
  Observable, pipe,
  switchMap,
  take,
  takeWhile,
  toArray
} from 'rxjs';
import {
  DataMeta,
  DataNodeType,
  NodeMeta,
  DataMetaKey,
  FileMetaData,
  DataSource
} from '../entities/responses/file-info-response';
import {Folder, FsNodeMeta} from '../components/fs/fs.component';
import {PermissionService} from './permission-service';

@Injectable({
  providedIn: 'root'
})
export class FsService {

  // private url = 'http://127.0.0.1:8080/auth/api/';
  // private url = document.location.hostname + '/api/auth/api/';
  // private url = 'http://127.0.0.1:8080/virtual_fs';
  // private url = 'https://10.42.0.212:8081';
  private url = 'https://158.160.98.131:8081';

  constructor(private client: HttpClient, private authService: AuthService) { }


  public createFile(path: string, files: File[]) {
    let token = this.authService.getToken();

    for (let file of files) {
      this.client.put(this.url + "/virtual_fs/file" + path + "/" + file.name, file, {
        headers: {
          "Authorization": token == null ? "" : token
        }
      }).subscribe(
        (r)=>{console.log('got r', r)}
      )
    }
  }

  public createFolder(rootFolder: string, folderName: string) {
    let token = this.authService.getToken();

    this.client.post(this.url + '/virtual_fs/folder' + rootFolder + '/' + folderName, {}, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    }).subscribe(
      (r)=>{console.log('got r', r)}
    )
  }

  makeid(length: number) {
    let result           = '';
    let characters       = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    let charactersLength = characters.length;
    for ( let i = 0; i < length; i++ ) {
      result += characters.charAt(Math.floor(Math.random() * charactersLength));
    }
    return result;
  }

  public getDataSource(totalSize: number, dataId: string) {
    let token = this.authService.getToken();

    return this.client.get(this.url + "/virtual_fs/file/data_meta/" + dataId, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    }).pipe(map(response => {
      let dataMeta = response as DataMeta;

      if (dataMeta.data_type == DataNodeType.Leaf) {
        return new DataSource([0, totalSize], dataMeta.key)
        // return [[0, totalSize], dataMeta.key];
      } else {
        return new DataSource([0, totalSize], dataMeta.key)
        // return [[0, totalSize], dataMeta.key];
      }
    }))
  }

  public getFile(path: string, progress: ((progress: number, total: number) => void)) {
    let token = this.authService.getToken();


    let task: Observable<string | void> = this.client.get(this.url + "/virtual_fs/file" + path, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    }).pipe(flatMap(response => {
      let json = response as NodeMeta;

       if (json.File != null) {
         let observables: Observable<DataSource>[] = [];
         for (let chunk of json.File.data) {
           let fileMeta = (chunk[1] as FileMetaData)
           let path = fileMeta.hash + "_" + fileMeta.hash_local_id;

           let range = chunk[0] as number[];
            let data: Observable<DataSource> = this.getDataSource(range[1] - range[0], path)
            observables.push(data)
         }
         return merge(observables);
         // return data.pipe(take(100), toArray());
       } else {
         return EMPTY;
       }
    }))
      .pipe(concatMap(v => v as Observable<DataSource>))
      .pipe(take(100), toArray())
      .pipe(map(async chunks => {
      let tempId = this.makeid(5);
      return await navigator.storage.getDirectory()
        .then(async root => {
          let file = await root.getFileHandle("temp_" + tempId, {
            create: true

          })
          let writable = await file.createWritable({
            keepExistingData: true,
            mode: "siloed"
          } as FileSystemCreateWritableOptions);

          chunks.sort((a, b) => a.range[0] - b.range[0])
          let total = chunks[chunks.length - 1].range[1];
          for (let chunk of chunks) {
            await fetch(this.url + '/virtual_fs/file/data/' + chunk.key.hash + "_" +  chunk.key.hash_local_id, {
              headers: {
                "Authorization": token == null ? "" : token
              }
            }).then(async (value) => {
              if (value.body != null) {


                let reader = await value.body?.getReader();

                // stream.pipeline(
                //   reader.getReader,
                //   new stream.Transform({
                //     transform(chunk, encoding, callback) {
                //       totalBytes += chunk.length;
                //       console.log(totalBytes);
                //       this.push(chunk);
                //       callback();
                //     }
                //   }),
                //   writable
                // )


                /*
                //
                let t = await reader.read();
                while (t.done) {
                  console.log(t.value);
                  // await writable.write(t.value as Uint8Array);
                  t = await reader.read();
                  // await value.body?.pipeTo(async (v) => {
                  //   await writable.write(v);
                  // });
                  //  {
                  //   preventClose: true
                  // });

                }


                 */

                let readRes = await reader.read();
                let totalWrite = 0;
                while (!readRes.done) {

                  await writable.write(readRes.value)
                  totalWrite += readRes.value.length;
                  progress.apply(null, [totalWrite, total]);

                  readRes = await reader.read();
                }


                /*
                reader.read().then(function processText({ done, value }): any {
                  // Result objects contain two properties:
                  // done  - true if the stream has already given you all its data.
                  // value - some data. Always undefined when done is true.
                  if (done) {
                    // logConsumer(`readStream() complete. Total bytes: ${bytesReceived}`);
                    return;
                  }
                  console.log(done)
                  console.log(value)

                  // bytesReceived += value.length;
                  // logConsumer(
                  //   `Read ${value.lengt  h} (${bytesReceived}). Current bytes = ${value}`,
                  // );
                  // result += value;

                  // Read some more, and call this function again
                  return reader.read().then(processText);
                });

                 */

                // for await (const chunk of reader) {
                //   await writable.getWriter().write(chunk);
                // }

                // reader.on('data', (chunk) => {
                //   bytesCopied += chunk.length;
                //   const progress = (bytesCopied / size) * 100;
                //   console.log(`Progress: ${progress.toFixed(2)}%`);
                // });
                //
                // Use pipeline to handle the stream copying
                // await pipelinePromise(readStream, writeStream);


                // pipe()
                // value.body?.pipeThrough(new ProgressPipe(null))
                //
                // await value.body?.pipeTo(new ProgressPipe(null, writable), {
                //   preventClose: true
                // });

              }
            })
          }
          await writable.close();
          console.log("closed")

          // await writable.close();
          return file.name;
        });
    })).pipe(
      switchMap(promise => promise));


    return task;

/*
    let tempId = this.makeid(5);
    let root = navigator.storage.getDirectory()
      .then(async root => {
        let file = await root.getFileHandle("temp_" + tempId, {
          create: true

        })
        let writable = await file.createWritable({
          keepExistingData: true,
          mode: "siloed"
        } as FileSystemCreateWritableOptions);

        await writable.truncate(0);
        await writable.close();



        return fetch('https://10.42.0.212:8080/virtual_fs/file/data/a2def047a73941e01a73739f92755f86b895811afb1f91243db214cff5bdac3f_6Dq13', {
          headers: {
            "Authorization": token == null ? "" : token
          }
        }).then(async (value) => {
          // const blob = await value.blob();
          // let writable = await file.createSyncAccessHandle();


          // let writable = await file.createWritable({
          //   keepExistingData: true,
          //   mode: "siloed"
          // } as FileSystemCreateWritableOptions);

          // const buf = new ArrayBuffer(1000);
          // for(let i = 0; i < 2; i++) {
          //   await writable.write(new Blob([buf]))
          // }


          let task = null;
          if (value.body != null) {
            // let sink = writable.getWriter();
            // value.body.pipeTo()
            let reader = value.body.getReader();
            let t = await reader.read()
            // while (t.done)

            // await value.body?.pipeTo(async (v) => {
            //   await writable.write(v);
            // },
            //  {
            //   preventClose: true
            // });
          }

          // await writable.seek(0);
          // let task = writable.write({
          //   data: blob,
          //   type: "write",
          //   position: 0
          // })


          // let writable2 = await file.createWritable({
          //   keepExistingData: true,
          //   mode: "siloed"
          // } as FileSystemCreateWritableOptions);
          // let pos = await writable2.seek(50);
          // let task2 = writable.write({
          //   data: blob,
          //   type: "write",
          //   position: blob.size + 10
          // })

          // let writable3 = await file.createWritable();
          // await writable3.seek(100);
          // let task3 = writable3.write(blob)
          //
          // let writable4 = await file.createWritable();
          // await writable4.seek(5500);
          // let task4 = writable4.write(blob)

          if (task != null) {
            await Promise.all([task]);
          }

          // await writable2.close()
          await writable.close()
          // console.log(blob.size)

          return file.name
        });
      })

    return root;

 */
  }

  public uploadFile(path: string, file: File) {
    let token = this.authService.getToken();
    return this.client.put(this.url + "/virtual_fs/file" + path + "/" + file.name, file, {
      headers: {
        "Authorization": token == null ? "" : token
      },
      reportProgress: true,  observe: 'events',
    });
  }

  public async getNodeMeta(path: string) {
    let token = this.authService.getToken();
    let node = await fetch(this.url + "/virtual_fs/meta/node" + path, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    });

    let textJson = (await node.text()).replace(/("[^"]*"\s*:\s*)(\d{16,})/g, '$1"$2"')
    let json = JSON.parse(textJson);
    // let json = await node.json();

    return json as FsNodeMeta;
  }

  authHeader() {
    let token = this.authService.getToken();
    if (token != undefined) {
      return {
        headers: {
          "Authorization": token
        }
      }
    }
    return {}
  }

  public async getFolder(path: string): Promise<Folder> {
    let token = this.authService.getToken();
    const response = await fetch(this.url + "/virtual_fs/folder" + path, this.authHeader());

    if (response.status == 403) {
      return {
        files: [], id: 0, is_partial: true, name: ''
      }
    }

    let json = await response.json();
    let folder = json as Folder;

    return folder;
  }
}



