import {Component, computed, model, resource, ViewEncapsulation} from '@angular/core';
import {FilePreviewOutput, Message, NgxVoyageComponent} from 'ngx-voyage';
import {UserInfoComponent} from '../user-info/user-info.component';
import {AuthService} from '../../services/auth.service';
import {CommonModule, Location} from '@angular/common';

import {NgxFileDropEntry, NgxFileDropModule} from 'ngx-file-drop';
import {HttpClient, HttpEventType} from '@angular/common/http';
import {Observable, timer} from 'rxjs';
import {FsService} from '../../services/fs-service';
import {PermissionManagementComponent} from '../permission-management/permission-management.component';
import {MatDialog} from '@angular/material/dialog';
import {Router} from '@angular/router';
import {ObjType} from '../../entities/obj-type';
import {PermissionService} from '../../services/permission-service';
import {MatProgressBar} from '@angular/material/progress-bar';
import {FileModificationComponent} from '../file-modification/file-modification.component';

@Component({
  selector: 'app-fs',
  imports: [NgxVoyageComponent, UserInfoComponent, NgxFileDropModule, MatProgressBar, CommonModule],
  templateUrl: './fs.component.html',
  styleUrl: './fs.component.css'
})
export class FsComponent {
  filesResource = resource({
    request: () => ({ path: encodeURIComponent(this.path()) }),
    loader: async ({ request }) => {

      this.isPartialFolder = false;
      this.location.go("/fs" + this.path())
      let folder = await this.fsService.getFolder(this.path());
      this.isPartialFolder = folder.is_partial;

      return folder;
    },
  });
  path = model('/');
  files = computed(() => this.filesResource.value()?.files.map((v) => {
    return {
      isDirectory: v.node_type == FsNodeType.Folder,
      isFile: v.node_type == FsNodeType.File,
      name: v.name,
      size: 1,
      isSymbolicLink: false,
      modifiedDate: new Date(),
    }
  }) ?? []);

  isPartialFolder = false;
  uploadingInProgress: Map<string, UserDataTransfer> = new Map<string, UserDataTransfer>();
  downloadInProgress: Map<string, UserDataTransfer> = new Map<string, UserDataTransfer>();


  message(): Message | undefined {
    if (this.isPartialFolder) {
      return {text: 'Содержимое папки может быть неполным. Нет прав на чтение всей папки', type: 'warn'};
    } else {
      return undefined
    }
  }

  async refresh() {
    console.log("refresh: " + this.path());
    let init_path = this.path();
    let folder = await this.fsService.getFolder(this.path());
    this.isPartialFolder = folder.is_partial;

    if (init_path == this.path()) {
      this.filesResource.set(folder);
    } else {
      console.log("skip refresh")
    }
  }

  constructor(private service: AuthService, private location: Location, private client: HttpClient, private fsService: FsService,
              private matDialog: MatDialog, private router: Router, private permissionService: PermissionService) {
    let current_route = this.router.url;
    if (current_route == "/fs/" || current_route == "/fs") {
      current_route = "/home/" + service.getLogin();
      location.go(current_route)
    } else {
      console.log(this.router.url);
      const regEx = new RegExp(/^\/fs/, 'gm');
      current_route = this.router.url.replace(regEx, "");
    }
    // this.downloadInProgress.set("CLion-2023.3.2.tar.gz", {
    //   lastUpdateTimestamp: 0, loaded: 20, progress: 30, speed: 3.12, title: 'CLion-2023.3.2.tar.gz'
    //
    // });
    console.log("current route: " + current_route);
    // this.path.set('/home/' + service.getLogin());
    this.path.set(current_route);
    // this.path.set(current_route.replace("^fs", ""));

    timer(10000, 10000)
      .subscribe(async () => {
        await this.refresh();
      })


    // const dropZone = document.body;
    // if (dropZone) {
    //   let hoverClassName = 'hover';
    //
    //   dropZone.addEventListener("dragenter", function(e) {
    //     e.preventDefault();
    //     dropZone.classList.add(hoverClassName);
    //   });
    //
    //   dropZone.addEventListener("dragover", function(e) {
    //     e.preventDefault();
    //     dropZone.classList.add(hoverClassName);
    //   });
    //
    //   dropZone.addEventListener("dragleave", function(e) {
    //     e.preventDefault();
    //     dropZone.classList.remove(hoverClassName);
    //   });
    //
    //   // Это самое важное событие, событие, которое дает доступ к файлам
    //   dropZone.addEventListener("drop", function(e) {
    //     e.preventDefault();
    //     dropZone.classList.remove(hoverClassName);
    //
    //     const files = Array.from(e.dataTransfer?.files);
    //     console.log(files);
    //
    //
    //
    //     // TODO что-то делает с файлами...
    //   });
    // }
  }


  getToken() {
    return this.service.getToken();
  }

  openModifyFileWindow(path: string) {
    console.log("openModifyFileWindow" + path)
    this.matDialog.open(FileModificationComponent, {
      data: {
        filename: path,
        // objId: node.id,
        // objType: objType,
        // permissions: permissions,
        // isPublic: false
      }
    })
  }

  deleteFile(path: string) {
    console.log("delete file " + path)
    this.fsService.deleteFile(path);
  }

  async openPermissionManagement(path: string) {
    console.log("openPermissionManagement" + path)

    // let node = await this.fsService.getNodeMeta(path);
    // let objType;
    // switch (node.file_type) {
    //   case FsNodeType.File:
    //     objType = ObjType.File;
    //     break;
    //   case FsNodeType.Folder:
    //     objType = ObjType.Folder;
    //     break;
    // }
    // console.log(node)
    // console.log(objType)
    //
    // let permissions = await this.permissionService.getPermissions(objType, node.id);

    this.matDialog.open(PermissionManagementComponent, {
      data: {
        path: path,
        // objId: node.id,
        // objType: objType,
        // permissions: permissions,
        // isPublic: false
      }
    })
  }

  openFile(path: string) {
    console.log("open" + path)
    // const url = encodeURIComponent(path);

    let filename: string = path.replace(/^.*[\\/]/, '')
    let progressFunc = (progress: number, total: number) => {
      this.updateProgress(progress, total, this.downloadInProgress, filename)
    }

    let transfer: UserDataTransfer = {
      lastUpdateTimestamp: Date.now(),
      loaded: 0, progress: 0, speed: 0,
      title: filename
    }
    // let filepath = this.path() + path;

    this.downloadInProgress.set(filename, transfer)

    this.fsService.getFile(path, progressFunc)
      .subscribe(async downloadedFile => {

        this.downloadInProgress.delete(filename);


        if (typeof downloadedFile == "string") {
          let root = await navigator.storage.getDirectory()

          const fileHandle = await root.getFileHandle(downloadedFile);
          console.log(downloadedFile)
          console.log(fileHandle.name)
          console.log((await fileHandle.getFile()).size)
          this.downloadBlob(await fileHandle.getFile(), path.slice(path.lastIndexOf("/") + 1))
        } else {
          console.log("downloaded file is null")
        }
      })
  }

  getFileContent({ path, cb }: FilePreviewOutput) {
    // const url = encodeURIComponent(path);

    let filename: string = path.replace(/^.*[\\/]/, '')

    let progressFunc = (progress: number, total: number) => {
      this.updateProgress(progress, total, this.downloadInProgress, filename)
    }
    let transfer: UserDataTransfer = {
      lastUpdateTimestamp: Date.now(),
      loaded: 0, progress: 0, speed: 0,
      title: filename
    }
    this.downloadInProgress.set(filename, transfer)

    this.fsService.getFile(path, progressFunc)
      .subscribe(async downloadedFile => {
        this.downloadInProgress.delete(filename);

        if (typeof downloadedFile == "string") {
          let root = await navigator.storage.getDirectory()

          const fileHandle = await root.getFileHandle(downloadedFile);
          console.log(downloadedFile, path.slice(path.lastIndexOf("/") + 1))
          console.log(fileHandle.name)
          console.log((await fileHandle.getFile()).size)
          cb(await fileHandle.getFile())

          console.log(cb)
          timer(100)
            .subscribe((_) => {
              let elements1 = document.getElementsByTagName("ngx-voyage-pdf");
              let elements2 = document.getElementsByTagName("ngx-voyage-img");
              // console.log(elements);
              let wrapper = elements1[0] == undefined ? elements2[0] : elements1[0];

              for (let i = 0; i < wrapper.children.length; i++) {
                let child = wrapper.children[i];
                console.log(child);

                // if (child instanceof ifram) {
                //   if ("property" in child["style"]) {
                Object(child).style = "width: 100%"
                  // }
                // }
                // child += "all_width"

                console.log(child);

                // ngx-voyage-img
              }
            })

        } else {
          console.log("downloaded file is null")
        }
      })
    /*
    fetch('http://localhost:8080/virtual_fs/file/data/4f04b7da4b7d65c8cb4c299c7287ae9dae5dad75b24988043bb0e576b8fd2cac_AW7Oz', {
      headers: {
        // "Authorization": token == null ? "" : token
      }
    }).then(async (value) => {
      const blob = await value.blob();
      cb(blob);
    });

     */
  }
  downloadBlob(blob: Blob, name = 'file.txt') {
    // Convert your blob into a Blob URL (a special url that points to an object in the browser's memory)
    const blobUrl = URL.createObjectURL(blob);

    // Create a link element
    const link = document.createElement("a");

    // Set link's href to point to the Blob URL
    link.href = blobUrl;
    link.download = name;

    // Append link to the body
    document.body.appendChild(link);

    // Dispatch click event on the link
    // This is necessary as link.click() does not work on the latest firefox
    link.dispatchEvent(
      new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        view: window
      })
    );

    // Remove link from body
    document.body.removeChild(link);
  }

  public async createFolder() {
    let folderName = window.prompt("Название папки");
    if (folderName == null) {
      return;
    }

    this.fsService.createFolder(this.path(), folderName == null ? "new_folder" : folderName)
    timer(1000)
      .subscribe(async () => {
        await this.refresh();
      })
  }

  public files2: NgxFileDropEntry[] = [];

  public isActiveDrop = 0;
  public dragstart(e: DragEvent) {
    // e.stopPropagation();
    e.preventDefault();
    // console.log(e);
  }

  isProgressBar(e: DragEvent) {
    if ("originalTarget" in e) {
      if ("className" in (e["originalTarget"] as any)) {
        let target = e["originalTarget"] as any;
        if ("attributes" in target) {
          if ("role" in target.attributes) {
            if ("value" in target.attributes.role) {
              if (target.attributes.role.value as string == 'progressbar') {
                return true;
              } else {
                return false;
              }
            }
          } else if ("class" in target.attributes) {
            if ("value" in target.attributes.class) {
              if ((target.attributes.class.value as string).includes('progressbar') ||
                (target.attributes.class.value as string).includes('message-wrapper')) {
                return true;
              } else {
                return false;
              }
            }
          }
        }
      }
    }
    return false;
  }

  public drageneter(e: DragEvent) {
    // e.stopPropagation();

    if (this.isProgressBar(e)) {
      return
    }
    e.preventDefault();

    this.isActiveDrop += 1;
    // console.log("start: " + this.isActiveDrop)
    // console.log(e)
  }

  public dragleave(e: DragEvent) {
    // e.preventDefault();
    if (this.isProgressBar(e)) {
      return
    }
    this.isActiveDrop -= 1;
    // console.log("end: " + this.isActiveDrop)
    // console.log(e)
  }

  public dragover(e: DragEvent) {
    e.preventDefault()
  }

  public drop123(e: DragEvent) {
    e.preventDefault();
    this.isActiveDrop = 0;

    if (e.dataTransfer != undefined) {
      const files = Array.from(e.dataTransfer.files);
      console.log(files);

      let transfer: UserDataTransfer = {
        lastUpdateTimestamp: Date.now(),
        loaded: 0, progress: 0, speed: 0,
        title: files[0].name
      }
      let filepath = this.path() + files[0].name;
      this.uploadingInProgress.set(filepath, transfer);
      console.log(this.uploadingInProgress);

      this.fsService.uploadFile(this.path(), files[0]).subscribe(
        async (event) => {
          switch (event.type) {
            case HttpEventType.UploadProgress:
              let total = event.total == undefined ? 1 : event.total;
              this.updateProgress(event.loaded, total, this.uploadingInProgress, filepath)
              /*

              let prevRecord = this.uploadingInProgress.get(filepath);
              if (prevRecord == undefined) {
                return;
              }

              let now = Date.now();
              let timeDelta = now - prevRecord.lastUpdateTimestamp;
              let dataDelta = event.loaded - prevRecord.loaded;
              let speed = (dataDelta / 1024 / 1024 / timeDelta) * 1000;

              let prevSpeed = prevRecord.speed;
              let speedDelta = speed - prevSpeed;
              let resultSpeed = prevSpeed + (speedDelta * 0.25);

              prevRecord.progress = event.loaded / total * 100;
              prevRecord.loaded = event.loaded;
              prevRecord.speed = resultSpeed;
              prevRecord.lastUpdateTimestamp = now;



               */
              break;
            case HttpEventType.Response:
              this.uploadingInProgress.delete(filepath);
              console.log('got r', event.body)
              timer(1000)
                .subscribe(async () => {
                  await this.refresh();
                });
              break;
            // await this.refresh();
          }
        }
      )

    } else {
      console.log("datatransfer is null")
      console.log(e)
    }
  }

  public dropped(files: NgxFileDropEntry[]) {
    this.files2 = files;
    for (const droppedFile of files) {

      // Is it a file?
      if (droppedFile.fileEntry.isFile) {
        const fileEntry = droppedFile.fileEntry as FileSystemFileEntry;
        fileEntry.file((file: File) => {

          // let index = this.uploadingInProgress.push([file.name, 0]);
          console.log(this.uploadingInProgress);
          // Here you can access the real file
          console.log(droppedFile.relativePath, file);
          this.fsService.uploadFile(this.path(), file).subscribe(
            async (event) => {
              switch (event.type) {
                case HttpEventType.UploadProgress:
                  // this.uploadingInProgress[index].loaded = event.loaded;
                  console.log(this.uploadingInProgress);
                  break;
                case HttpEventType.Response:
                  console.log('got r123', event.body)
                  timer(1000)
                    .subscribe(async () => {
                      await this.refresh();
                    });
                  break;
                // await this.refresh();
              }
            }
          )

          // let token = this.getToken();
          // this.client.put("http://localhost:8081/virtual_fs/file" + this.path() + "/" + file.name, file, {
          //   headers: {
          //     "Authorization": token == null ? "" : token
          //   }
          // }).subscribe(
          //   async (r) => {
          //     console.log('got r', r)
          //     await this.refresh();
          //   }
          // )

          /**
           // You could upload it like this:
           const formData = new FormData()
           formData.append('logo', file, relativePath)

           // Headers
           const headers = new HttpHeaders({
           'security-token': 'mytoken'
           })

           this.http.post('https://mybackend.com/api/upload/sanitize-and-save-logo', formData, { headers: headers, responseType: 'blob' })
           .subscribe(data => {
           // Sanitized logo returned from backend
           })
           **/

        });
      } else {
        // It was a directory (empty directories are added, otherwise only files)
        const fileEntry = droppedFile.fileEntry as FileSystemDirectoryEntry;
        console.log(droppedFile.relativePath, fileEntry);
      }
    }
  }

  updateProgress(progress: number, total: number, storageMap: Map<string, UserDataTransfer>, path: string) {
    let prevRecord = storageMap.get(path);
    if (prevRecord == undefined) {
      return;
    }

    let now = Date.now();
    let timeDelta = now - prevRecord.lastUpdateTimestamp;
    if (timeDelta < 50) {
      return;
    }
    let dataDelta = progress - prevRecord.loaded;
    let speed = (dataDelta / 1024 / 1024 / timeDelta) * 1000;

    let prevSpeed = prevRecord.speed;
    let speedDelta = speed - prevSpeed;
    let resultSpeed = prevSpeed + (speedDelta * 0.25);

    prevRecord.progress = progress / total * 100;
    prevRecord.loaded = progress;
    prevRecord.speed = resultSpeed;
    prevRecord.lastUpdateTimestamp = now;
  }

  public fileOver(event: any){
    console.log(event);
  }

  public fileLeave(event: any){
    console.log(event);
  }

}

export interface FsNodeMeta {
  filename: string
  file_type: FsNodeType
  id: string
  keepers: FileKeeper[]
}

export interface FileKeeper {
  id: NodeId,
  data: StoredFile[]
}

export interface StoredFile {
  version: number,
  is_keeper: boolean
}

export interface NodeId {
  ip: string,
  port: number
}

export interface Folder {
  name: string
  files: FsNode[]
  is_partial: boolean
  id: number
}
export interface FsNode {
  name: string;
  node_type: FsNodeType;
}

export enum FsNodeType {
  File = "File",
  Folder = "Folder"
}

class UserDataTransfer {
  title: string;
  progress: number;
  loaded: number;
  speed: number;
  lastUpdateTimestamp: number


  constructor(title: string, progress: number, loaded: number, speed: number, lastUpdateTimestamp: number) {
    this.title = title;
    this.progress = progress;
    this.loaded = loaded;
    this.speed = speed;
    this.lastUpdateTimestamp = lastUpdateTimestamp;
  }
}
