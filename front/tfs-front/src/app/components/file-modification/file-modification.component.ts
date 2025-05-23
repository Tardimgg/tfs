import {Component, Inject} from '@angular/core';
import {MAT_DIALOG_DATA, MatDialogModule, MatDialogRef} from '@angular/material/dialog';
import {PermissionService} from '../../services/permission-service';
import {MatSelectModule} from '@angular/material/select';
import {CommonModule, NgFor} from '@angular/common';
import {MatButtonModule} from '@angular/material/button';
import {MatInputModule} from '@angular/material/input';
import {FormsModule} from '@angular/forms';
import {castFileChangeTypeToE, FileChange, FileChangeType} from '../../entities/file-change';
import {timer} from 'rxjs';
import {FsService} from '../../services/fs-service';
import {FileMeta} from '../../entities/responses/file-info-response';

@Component({
  selector: 'app-file-modification',
  imports: [MatDialogModule, MatSelectModule, NgFor, MatButtonModule, CommonModule, MatInputModule, FormsModule],
  templateUrl: './file-modification.component.html',
  styleUrl: './file-modification.component.css'
})
export class FileModificationComponent {

  operation: string = "add";
  filename: string;
  filesize: bigint;
  changes: [FileChange] = [new FileChange(FileChangeType.Add)]
  constructor(
    public dialogRef: MatDialogRef<FileModificationComponent>,
    @Inject(MAT_DIALOG_DATA) public data: object,
    public permissionService: PermissionService,
    private fsService: FsService) {
    this.dialogRef.updateSize('60%', '65%');

    this.filename = "filename" in data ? data["filename"] as string : "";
    this.filesize = BigInt(0);

    timer(0)
      .subscribe(async(_) => {
        this.fsService.getFileMeta(this.filename)
          .subscribe((response) => {
            let json = response as FileMeta;

            let totalSize = 0;
            for (let chunk of json.data) {
              let range = chunk[0] as number[];
              totalSize += range[1] - range[0];
            }

            this.filesize = BigInt(totalSize);
          })
        // let node = await this.fsService.getNodeMeta(this.filename);

        // node.

      });
  }



  save() {

  }

  protected readonly castFileChangeTypeToE = castFileChangeTypeToE;
  protected readonly BigInt = BigInt;
}
