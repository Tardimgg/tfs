import {ChangeDetectorRef, Component, Inject} from '@angular/core';
import {MAT_DIALOG_DATA, MatDialogModule, MatDialogRef} from '@angular/material/dialog';
import {PermissionService} from '../../services/permission-service';
import {MatSelectModule} from '@angular/material/select';
import {CommonModule, NgFor} from '@angular/common';
import {MatButtonModule} from '@angular/material/button';
import {MatInputModule} from '@angular/material/input';
import {FormsModule} from '@angular/forms';

@Component({
  selector: 'app-file-modification',
  imports: [MatDialogModule, MatSelectModule, NgFor, MatButtonModule, CommonModule, MatInputModule, FormsModule],
  templateUrl: './file-modification.component.html',
  styleUrl: './file-modification.component.css'
})
export class FileModificationComponent {

  filename: string;
  constructor(
    public dialogRef: MatDialogRef<FileModificationComponent>,
    @Inject(MAT_DIALOG_DATA) public data: object,
    public permissionService: PermissionService) {
    this.dialogRef.updateSize('60%', '60%');

    this.filename = "filename" in data ? data["filename"] as string : "";


  }

  save() {

  }
}
