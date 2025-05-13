import {ChangeDetectorRef, Component, Inject} from '@angular/core';
import {MAT_DIALOG_DATA, MatDialogModule, MatDialogRef} from '@angular/material/dialog';
import {MatSelectModule} from '@angular/material/select';
import {NgFor} from '@angular/common';
import {MatButtonModule} from '@angular/material/button';
import {PermissionType} from '../../entities/permission-type';
import {castToE, PermissionService} from '../../services/permission-service';
import {ObjType} from '../../entities/obj-type';

@Component({
  selector: 'app-permission-controller',
  imports: [MatDialogModule, MatSelectModule, NgFor, MatButtonModule],
  templateUrl: './permission-controller.component.html',
  styleUrl: './permission-controller.component.css'
})
export class PermissionControllerComponent {
  displayedPermissions: PermissionType[];
  displayedNewPermissions: PermissionWrapper[] = []
  permissionsToDelete: PermissionType[] = []

  filename: string
  objId: string;
  objType: ObjType;

  userLogin: string;

  allPermissions: PermissionType[] = []

  uniqIndex = 0;

  constructor(
    public dialogRef: MatDialogRef<PermissionControllerComponent>,
    @Inject(MAT_DIALOG_DATA) public data: object,
    public permissionService: PermissionService,   private cdr: ChangeDetectorRef) {
    this.dialogRef.updateSize('30%', '60%');


    this.filename = "filename" in data ? data["filename"] as string : "";
    this.objId = "objId" in data ? data["objId"] as string : "";
    this.objType = "objType" in data ? data["objType"] as ObjType : ObjType.File;

    this.userLogin = "userLogin" in data ? data["userLogin"] as ObjType : ObjType.File;

    this.displayedPermissions = "userPermissions" in data ? data["userPermissions"] as PermissionType[] : [];

    // console.log(Object.keys(PermissionType));
    this.allPermissions = [
      PermissionType.Read,
      PermissionType.Write,
      PermissionType.GrantRead,
      PermissionType.GrantWrite,
      PermissionType.ReadRecursively,
      PermissionType.WriteRecursively,
      PermissionType.GrantReadRecursively,
      PermissionType.GrantWriteRecursively
    ]
  }

  closeDialog() {
    let newPermissions: PermissionType[] = [];
    for (let permission of this.displayedNewPermissions) {
      newPermissions.push(permission.permissionType);
    }

    let toDelete = this.permissionsToDelete.filter((v) => newPermissions.indexOf(v) == -1);

    this.dialogRef.close({
      toDelete: [...new Set(toDelete)],
      toAdd: [...new Set(newPermissions)]
    });
  }

  markAsDeleted(index: number): void {
    this.permissionsToDelete.push(this.displayedPermissions[index])
    this.displayedPermissions.splice(index, 1);
  }

  changeNewPermission(index: number, newPermission: PermissionType): void {
    this.displayedNewPermissions[index].permissionType = newPermission;
  }

  deleteNewPermission(index: number): void {
    this.displayedNewPermissions.splice(index, 1);
  }

  clickAddPermission(): void {
    this.displayedNewPermissions.push({
      id: this.uniqIndex, permissionType: PermissionType.Read
    })
    this.uniqIndex++;
  }

  protected readonly castToE = castToE;
}

class PermissionWrapper {
  permissionType: PermissionType;
  id: number


  constructor(permissionType: PermissionType, id: number) {
    this.permissionType = permissionType;
    this.id = id;
  }
}
